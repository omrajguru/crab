use futures::lock::Mutex;
use serde::{Deserialize, Serialize};

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use futures::channel::oneshot;
use serde_json::{json, Value};
use worker::*;

use crate::mcp::{self, Call, Parsed};
use crate::model::{self, Body, RateWindow, Role, Task};
use crate::tools::{
    self, CancelTaskArgs, GetNextTaskArgs, GetResultArgs, ListTasksArgs, SubmitResultArgs,
    SubmitTaskArgs,
};

type Waiter = oneshot::Sender<Option<Rc<String>>>;

type Res<T> = std::result::Result<T, String>;

fn now_ms() -> u64 {
    Date::now().as_millis()
}

#[derive(Default, Clone, Serialize, Deserialize)]
struct Counters {
    created: u64,
    completed: u64,
    expired: u64,
    failed: u64,
}

#[derive(Default, Clone, Serialize, Deserialize)]
struct Index {
    tasks: Vec<Task>,
    counters: Counters,
}
impl std::ops::Deref for Index {
    type Target = Vec<Task>;
    fn deref(&self) -> &Vec<Task> {
        &self.tasks
    }
}
impl std::ops::DerefMut for Index {
    fn deref_mut(&mut self) -> &mut Vec<Task> {
        &mut self.tasks
    }
}

fn new_id(index: &[Task]) -> Res<String> {
    for _ in 0..8 {
        let mut bytes = [0u8; 5];
        getrandom::getrandom(&mut bytes).map_err(|_| "Unable to allocate a task ID. Retry.")?;
        let id = model::id_from_bytes(&bytes);
        if !index.iter().any(|t| t.id == id) {
            return Ok(id);
        }
    }
    Err("Unable to allocate a unique task ID. Retry.".into())
}

#[durable_object]
pub struct Relay {
    state: State,
    lock: Mutex<()>,
    waiters: RefCell<HashMap<String, Vec<Waiter>>>,
    orch_limit: RefCell<RateWindow>,
    work_limit: RefCell<RateWindow>,
    lease_ms: u64,
    tick_ms: u64,
}

impl DurableObject for Relay {
    fn new(state: State, env: Env) -> Self {
        let lease_ms = match env.var("LEASE_SECONDS") {
            Ok(v) => match v.to_string().trim().parse::<u64>() {
                Ok(seconds) if (1..=900).contains(&seconds) => seconds * 1000,
                _ => model::LEASE_MS,
            },
            _ => model::LEASE_MS,
        };
        Relay {
            state,
            lock: Mutex::new(()),
            waiters: RefCell::new(HashMap::new()),
            orch_limit: RefCell::new(RateWindow::new()),
            work_limit: RefCell::new(RateWindow::new()),
            lease_ms,
            tick_ms: lease_ms.min(model::ALARM_TICK_MS),
        }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        let path = req.path();
        let role = match path.as_str() {
            "/status" | "/rpc/orch" => Role::Orchestrator,
            "/rpc/work" => Role::Worker,
            _ => return Response::error("Unknown path", 404),
        };
        let now = now_ms();
        let allowed = match role {
            Role::Orchestrator => self.orch_limit.borrow_mut().allow(now),
            Role::Worker => self.work_limit.borrow_mut().allow(now),
        };
        if !allowed {
            return Response::error("Rate limit reached; retry in a minute.", 429);
        }

        if path == "/status" {
            let _guard = self.lock.lock().await;
            let index = self.load_index().await.map_err(Error::RustError)?;
            let mut summary = model::status_summary(&index, now_ms());
            summary["counters"] = json!(index.counters);
            return Response::from_json(&summary);
        }
        let body = req.text().await?;
        let parsed: Value = match serde_json::from_str(&body) {
            Ok(v) => v,
            _ => {
                let reply = mcp::err(&Value::Null, mcp::PARSE_ERROR, "Parse error");
                return Ok(Response::from_json(&reply)?.with_status(400));
            }
        };
        let batch = parsed.is_array();
        let messages: Vec<Value> = match parsed {
            Value::Array(items) => items,
            other => vec![other],
        };
        if messages.is_empty() || messages.len() > 4 {
            let reply = mcp::err(&Value::Null, mcp::INVALID_REQUEST, "Invalid request");
            return Ok(Response::from_json(&reply)?.with_status(400));
        }

        if batch
            && messages.iter().any(|m| {
                m.pointer("/params/arguments/wait_seconds")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0)
                    > 0.0
            })
        {
            return Response::from_json(&mcp::err(
                &Value::Null,
                mcp::INVALID_PARAMS,
                "Use a single request for long polling",
            ));
        }
        let mut replies: Vec<Value> = Vec::new();
        for message in &messages {
            for reply in self.handle_message(role, message).await {
                replies.push(reply);
            }
        }
        if replies.is_empty() {
            return Ok(Response::empty()?.with_status(202));
        }
        if batch {
            Response::from_json(&replies)
        } else {
            Response::from_json(&replies[0])
        }
    }

    async fn alarm(&self) -> Result<Response> {
        let _guard = self.lock.lock().await;
        let mut index = self.load_index().await.map_err(Error::RustError)?;
        self.maintain(&mut index).await.map_err(Error::RustError)?;
        self.reschedule(&index).await.map_err(Error::RustError)?;
        Response::ok("ok")
    }
}

impl Relay {
    async fn handle_message(&self, role: Role, message: &Value) -> Vec<Value> {
        match mcp::parse_message(message) {
            Parsed::Silent => Vec::new(),
            Parsed::Invalid(reply) => vec![reply],
            Parsed::Request(call) => vec![self.dispatch(role, call).await],
        }
    }

    async fn dispatch(&self, role: Role, call: Call) -> Value {
        let Call { id, method, params } = call;
        match method.as_str() {
            "initialize"
                if !params["protocolVersion"].is_string()
                    || !params["capabilities"].is_object()
                    || !params["clientInfo"]["name"].is_string()
                    || !params["clientInfo"]["version"].is_string() =>
            {
                mcp::err(
                    &id,
                    mcp::INVALID_PARAMS,
                    "Supply protocolVersion, capabilities and clientInfo",
                )
            }
            "initialize" => mcp::ok(&id, mcp::initialize_result(&params, role)),
            "ping" => mcp::ok(&id, json!({})),
            "tools/list" => mcp::ok(&id, tools::tool_list(role)),
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if !tools::exists(role, &name) {
                    return mcp::err(&id, mcp::INVALID_PARAMS, "Unknown tool for this endpoint");
                }
                let args = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let started = now_ms();
                let outcome = self.call_tool(&name, &args).await;
                let is_error = outcome.is_err();
                let text = match outcome {
                    Ok(t) => t,
                    Err(m) => m,
                };
                let task_id = args.get("task_id").and_then(Value::as_str).filter(|s| {
                    s.len() == 8
                        && s.bytes()
                            .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
                });
                console_log!(
                    "{}",
                    json!({"role":role.as_str(),"tool":name,"error":is_error,"ms":now_ms().saturating_sub(started),"task_id":task_id,"input_bytes":args.to_string().len(),"output_bytes":text.len()})
                );
                mcp::ok(&id, mcp::tool_result(text, is_error))
            }
            _ => mcp::err(&id, mcp::METHOD_MISSING, "Method missing"),
        }
    }

    async fn call_tool(&self, name: &str, args: &Value) -> Res<String> {
        let _guard = if name == "get_result" {
            None
        } else {
            Some(self.lock.lock().await)
        };
        match name {
            "submit_task" => self.submit_task(tools::parse_args(args)?).await,
            "get_result" => self.get_result(tools::parse_args(args)?).await,
            "list_tasks" => self.list_tasks(tools::parse_args(args)?).await,
            "cancel_task" => self.cancel_task(tools::parse_args(args)?).await,
            "get_next_task" => self.get_next_task(tools::parse_args(args)?).await,
            "submit_result" => self.submit_result(tools::parse_args(args)?).await,
            _ => Err("Unknown tool".to_string()),
        }
    }

    // Orchestrator tools

    async fn submit_task(&self, mut a: SubmitTaskArgs) -> Res<String> {
        let mut index = self.load_index().await?;
        self.maintain(&mut index).await?;
        crate::files::validate(&mut a.files)?;
        if let Some(key) = &a.request_id {
            if key.is_empty() || key.len() > 80 {
                return Err("Use a request_id of 1 to 80 bytes.".into());
            }
            if let Some(t) = index.iter().find(|t| t.request_id.as_ref() == Some(key)) {
                let body = self.load_body(&t.id).await?;
                if t.title != a.title.trim()
                    || t.target != a.target.trim()
                    || body.requirements != a.requirements
                    || body.context != a.context
                    || body.files != a.files
                {
                    return Err(
                        "This request_id already identifies different work. Use a new request_id."
                            .into(),
                    );
                }
                return Ok(json!({"task_id":t.id,"status":t.status}).to_string());
            }
        }
        if index.len() >= model::MAX_TASKS {
            return Err(format!(
                "The queue holds {} tasks; wait for finished tasks to expire after seven days.",
                model::MAX_TASKS
            ));
        }
        let (mut task, mut body) = model::new_task(
            new_id(&index)?,
            &a.title,
            &a.requirements,
            &a.context,
            &a.target,
            now_ms(),
        )?;
        let id = task.id.clone();
        task.request_id = a.request_id;
        body.files = a.files;
        index.push(task);
        index.counters.created += 1;
        self.commit(&index, Some((id.clone(), body)), vec![])
            .await?;
        Ok(json!({ "task_id": id, "status": "pending" }).to_string())
    }

    async fn get_result(&self, a: GetResultArgs) -> Res<String> {
        if !(0.0..=model::MAX_WAIT_SECONDS as f64).contains(&a.wait_seconds) {
            return Err("Use wait_seconds between 0 and 25.".into());
        }
        let deadline = now_ms() + (a.wait_seconds * 1000.0) as u64;
        loop {
            let guard = self.lock.lock().await;
            let index = self.load_index().await?;
            let i = model::find_index(&index, a.task_id.trim())?;
            let now = now_ms();
            if index[i].status.is_finished() || now >= deadline {
                let body = self.load_body(&index[i].id).await?;
                return Ok(model::result_view(&index[i], &body).to_string());
            }
            // Only synchronous code runs between the index read and the waiter
            // registration, so submit_result runs only after the waiter exists.
            let (tx, rx) = oneshot::channel();
            {
                let mut waiters = self.waiters.borrow_mut();
                let list = waiters.entry(index[i].id.clone()).or_default();
                list.retain(|s| !s.is_canceled());
                if list.len() >= 16 {
                    return Err("Too many collectors waiting for this task. Retry later.".into());
                }
                list.push(tx);
            }
            drop(guard);
            let remaining = Duration::from_millis(deadline - now);
            if let futures::future::Either::Left((Ok(Some(result)), _)) =
                futures::future::select(Box::pin(rx), Box::pin(Delay::from(remaining))).await
            {
                return Ok((*result).clone());
            }
        }
    }

    async fn list_tasks(&self, a: ListTasksArgs) -> Res<String> {
        let index = self.load_index().await?;
        let limit = a.limit.max(1.0) as usize;
        Ok(model::list_view(&index, &a.status, limit, now_ms())?.to_string())
    }

    async fn cancel_task(&self, a: CancelTaskArgs) -> Res<String> {
        let mut index = self.load_index().await?;
        let i = model::find_index(&index, a.task_id.trim())?;
        model::cancel(&mut index[i], now_ms())?;
        let id = index[i].id.clone();
        self.reschedule(&index).await?;
        self.save_index(&index).await?;
        self.wake(&id, None);
        Ok(json!({ "task_id": id, "status": "cancelled" }).to_string())
    }

    // Worker tools

    async fn get_next_task(&self, a: GetNextTaskArgs) -> Res<String> {
        let mut index = self.load_index().await?;
        self.maintain(&mut index).await?;
        let i = match model::pick_next(&index, &a.target) {
            Ok(i) => i,
            Err(message) => return Ok(message),
        };
        let body = self.load_body(&index[i].id).await?;
        model::claim(&mut index[i], now_ms(), self.lease_ms);
        let task = index[i].clone();
        self.reschedule(&index).await?;
        self.save_index(&index).await?;
        Ok(json!({
            "task_id": task.id,
            "title": task.title,
            "claim_number":task.attempts+1,
            "requirements": body.requirements,
            "context": body.context,
            "files": body.files,
            "instructions": tools::WORKER_INSTRUCTIONS,
        })
        .to_string())
    }

    async fn submit_result(&self, mut a: SubmitResultArgs) -> Res<String> {
        let mut index = self.load_index().await?;
        let i = model::find_index(&index, a.task_id.trim())?;
        let id = index[i].id.clone();
        if let Some(number) = a.claim_number {
            let expected = index[i].attempts + u32::from(index[i].status == model::Status::Claimed);
            if number == 0 || number != expected {
                return Err("This claim was superseded. Check for new work.".into());
            }
        }
        let mut body = self.load_body(&id).await?;
        crate::files::validate(&mut a.files)?;
        body.result_files = a.files;
        model::complete(&mut index[i], &mut body, &a.code, &a.summary, now_ms())?;
        let delivery = self
            .waiters
            .borrow()
            .contains_key(&id)
            .then(|| Rc::new(model::result_view(&index[i], &body).to_string()));
        index.counters.completed += 1;
        self.reschedule(&index).await?;
        self.commit(&index, Some((id.clone(), body)), vec![])
            .await?;
        self.wake(&id, delivery);
        Ok(json!({ "task_id": id, "status": "done" }).to_string())
    }

    // Waiters and alarms

    fn wake(&self, id: &str, result: Option<Rc<String>>) {
        let list = self.waiters.borrow_mut().remove(id);
        if let Some(senders) = list {
            for tx in senders {
                let _ = tx.send(result.clone());
            }
        }
    }

    async fn reschedule(&self, index: &[Task]) -> Res<()> {
        let ms = model::next_alarm_ms(index, now_ms(), self.tick_ms);
        if ms == 0 {
            return Ok(());
        }
        let storage = self.state.storage();
        if storage
            .get_alarm()
            .await
            .map_err(|e| e.to_string())?
            .is_some_and(|old| old > now_ms() as i64 && old <= (now_ms() + ms) as i64)
        {
            return Ok(());
        }
        storage
            .set_alarm(Duration::from_millis(ms))
            .await
            .map_err(|e| e.to_string())
    }

    // Storage

    async fn maintain(&self, index: &mut Index) -> Res<()> {
        let now = now_ms();
        let changed = model::expire_claims(&mut index.tasks, now);
        index.counters.expired += changed.len() as u64;
        index.counters.failed += index
            .tasks
            .iter()
            .filter(|t| t.status == model::Status::Failed && changed.contains(&t.id))
            .count() as u64;
        let removed = model::purge(&mut index.tasks, now);
        if !changed.is_empty() || !removed.is_empty() {
            self.reschedule(index).await?;
            self.commit(index, None, removed).await?;
            for id in changed {
                self.wake(&id, None);
            }
        }
        Ok(())
    }

    async fn load_index(&self) -> Res<Index> {
        let values = self
            .state
            .storage()
            .get_multiple(vec!["index"])
            .await
            .map_err(|e| e.to_string())?;
        let value = values.get(&"index".into());
        if value.is_undefined() {
            return Ok(Index::default());
        }
        if worker::js_sys::Array::is_array(&value) {
            let tasks = serde_wasm_bindgen::from_value(value).map_err(|e| e.to_string())?;
            return Ok(Index {
                tasks,
                ..Index::default()
            });
        }
        serde_wasm_bindgen::from_value(value).map_err(|e| e.to_string())
    }

    async fn save_index(&self, index: &Index) -> Res<()> {
        self.state
            .storage()
            .put("index", index)
            .await
            .map_err(|e| e.to_string())
    }
    async fn load_body(&self, id: &str) -> Res<Body> {
        self.state
            .storage()
            .get(&format!("b:{id}"))
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Task content is missing. Check relay storage.".into())
    }
    async fn commit(
        &self,
        index: &Index,
        body: Option<(String, Body)>,
        removed: Vec<String>,
    ) -> Res<()> {
        let index = index.clone();
        self.state
            .storage()
            .transaction(move |txn| async move {
                txn.put("index", index).await?;
                if let Some((id, body)) = body {
                    txn.put(&format!("b:{id}"), body).await?;
                }
                for id in removed {
                    txn.delete(&format!("b:{id}")).await?;
                }
                Ok(())
            })
            .await
            .map_err(|e| e.to_string())
    }
}
