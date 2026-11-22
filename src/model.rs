use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const MAX_TITLE: usize = 120;
pub const MAX_REQUIREMENTS: usize = 60_000;
pub const MAX_CONTEXT: usize = 120_000;
pub const MAX_CODE: usize = 200_000;
pub const MAX_SUMMARY: usize = 5_000;
pub const MAX_TARGET: usize = 40;
pub const MAX_BODY: usize = 1_000_000;
pub const MAX_TASKS: usize = 500;
pub const MAX_WAIT_SECONDS: u64 = 25;
pub const MAX_LIST_LIMIT: usize = 100;
pub const LEASE_MS: u64 = 15 * 60 * 1000;
pub const ALARM_TICK_MS: u64 = 60 * 1000;
pub const MAX_ATTEMPTS: u32 = 3;
pub const RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1000;
pub const RATE_LIMIT_PER_MINUTE: u32 = 120;
pub const QUEUE_EMPTY: &str = "The queue is empty.";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Orchestrator,
    Worker,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Orchestrator => "orch",
            Role::Worker => "work",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pending,
    Claimed,
    Done,
    Failed,
    Cancelled,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Claimed => "claimed",
            Status::Done => "done",
            Status::Failed => "failed",
            Status::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Result<Status, String> {
        match s {
            "pending" => Ok(Status::Pending),
            "claimed" => Ok(Status::Claimed),
            "done" => Ok(Status::Done),
            "failed" => Ok(Status::Failed),
            "cancelled" => Ok(Status::Cancelled),
            _ => Err(format!(
                "Unknown status {s}; use pending, claimed, done, failed, or cancelled."
            )),
        }
    }

    pub fn is_finished(self) -> bool {
        matches!(self, Status::Done | Status::Failed | Status::Cancelled)
    }
}

/// Light record kept in the index. An empty target string means any worker.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    #[serde(default)]
    pub request_id: Option<String>,
    pub id: String,
    pub title: String,
    pub target: String,
    pub status: Status,
    pub attempts: u32,
    pub created_at: u64,
    pub updated_at: u64,
    pub claimed_at: u64,
    pub completed_at: u64,
    pub lease_expires_at: u64,
}

/// Heavy text fields, stored under a separate key per task.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Body {
    #[serde(default)]
    pub files: Vec<crate::files::Attachment>,
    #[serde(default)]
    pub result_files: Vec<crate::files::Attachment>,
    pub requirements: String,
    pub context: String,
    pub code: String,
    pub summary: String,
}

fn check_len(field: &str, value: &str, max: usize) -> Result<(), String> {
    let len = value.chars().count();
    if len > max {
        return Err(format!(
            "The {field} field holds {len} characters; the limit is {max}."
        ));
    }
    Ok(())
}

pub fn new_task(
    id: String,
    title: &str,
    requirements: &str,
    context: &str,
    target: &str,
    now: u64,
) -> Result<(Task, Body), String> {
    if title.trim().is_empty() {
        return Err("The title field is blank.".to_string());
    }
    if requirements.trim().is_empty() {
        return Err("The requirements field is blank.".to_string());
    }
    check_len("title", title, MAX_TITLE)?;
    check_len("requirements", requirements, MAX_REQUIREMENTS)?;
    check_len("context", context, MAX_CONTEXT)?;
    check_len("target", target, MAX_TARGET)?;
    let task = Task {
        request_id: None,
        id,
        title: title.trim().to_string(),
        target: target.trim().to_string(),
        status: Status::Pending,
        attempts: 0,
        created_at: now,
        updated_at: now,
        claimed_at: 0,
        completed_at: 0,
        lease_expires_at: 0,
    };
    let body = Body {
        files: Vec::new(),
        result_files: Vec::new(),
        requirements: requirements.to_string(),
        context: context.to_string(),
        code: String::new(),
        summary: String::new(),
    };
    Ok((task, body))
}

pub fn find_index(tasks: &[Task], id: &str) -> Result<usize, String> {
    match tasks.iter().position(|t| t.id == id) {
        Some(i) => Ok(i),
        _ => Err(format!(
            "Unknown task id {id}. Call list_tasks to see open work."
        )),
    }
}

pub fn target_matches(task_target: &str, worker_target: &str) -> bool {
    task_target.is_empty() || task_target.eq_ignore_ascii_case(worker_target.trim())
}

/// Oldest pending task whose target suits the worker. The error text is the empty queue message.
pub fn pick_next(tasks: &[Task], worker_target: &str) -> Result<usize, String> {
    let mut best: Result<usize, String> = Err(QUEUE_EMPTY.to_string());
    let mut best_time = u64::MAX;
    for (i, t) in tasks.iter().enumerate() {
        if t.status == Status::Pending
            && target_matches(&t.target, worker_target)
            && t.created_at < best_time
        {
            best = Ok(i);
            best_time = t.created_at;
        }
    }
    best
}

pub fn claim(task: &mut Task, now: u64, lease_ms: u64) {
    task.status = Status::Claimed;
    task.claimed_at = now;
    task.lease_expires_at = now + lease_ms;
    task.updated_at = now;
}

pub fn complete(
    task: &mut Task,
    body: &mut Body,
    code: &str,
    summary: &str,
    now: u64,
) -> Result<(), String> {
    if task.status.is_finished() {
        return Err(format!(
            "Task {} is already {}; call get_next_task for open work.",
            task.id,
            task.status.as_str()
        ));
    }
    if code.trim().is_empty() && body.result_files.is_empty() {
        return Err("Send at least one complete file or a nonempty code string.".to_string());
    }
    check_len("code", code, MAX_CODE)?;
    check_len("summary", summary, MAX_SUMMARY)?;
    body.code = code.to_string();
    body.summary = summary.to_string();
    task.status = Status::Done;
    task.completed_at = now;
    task.updated_at = now;
    task.lease_expires_at = 0;
    Ok(())
}

pub fn cancel(task: &mut Task, now: u64) -> Result<(), String> {
    if task.status.is_finished() {
        return Err(format!(
            "Task {} is already {}.",
            task.id,
            task.status.as_str()
        ));
    }
    task.status = Status::Cancelled;
    task.completed_at = now;
    task.updated_at = now;
    task.lease_expires_at = 0;
    Ok(())
}

/// Returns expired claims to pending, or to failed once attempts run out. Gives back the ids that changed.
pub fn expire_claims(tasks: &mut [Task], now: u64) -> Vec<String> {
    let mut changed = Vec::new();
    for t in tasks.iter_mut() {
        if t.status == Status::Claimed && t.lease_expires_at <= now {
            t.attempts += 1;
            t.updated_at = now;
            t.lease_expires_at = 0;
            if t.attempts >= MAX_ATTEMPTS {
                t.status = Status::Failed;
                t.completed_at = now;
            } else {
                t.status = Status::Pending;
            }
            changed.push(t.id.clone());
        }
    }
    changed
}

/// Drops finished tasks past the retention window and gives back their ids.
pub fn purge(tasks: &mut Vec<Task>, now: u64) -> Vec<String> {
    let mut removed = Vec::new();
    tasks.retain(|t| {
        let stale = t.status.is_finished() && now.saturating_sub(t.updated_at) >= RETENTION_MS;
        if stale {
            removed.push(t.id.clone());
        }
        !stale
    });
    removed
}

/// Delay in milliseconds until the next alarm; zero means the queue is idle.
pub fn next_alarm_ms(tasks: &[Task], now: u64, tick_ms: u64) -> u64 {
    if tasks.iter().any(|t| t.status == Status::Claimed) {
        return tasks
            .iter()
            .filter(|t| t.status == Status::Claimed)
            .map(|t| t.lease_expires_at.saturating_sub(now).max(1))
            .min()
            .unwrap_or(tick_ms)
            .min(tick_ms);
    }
    let mut soonest = 0u64;
    for t in tasks.iter().filter(|t| t.status.is_finished()) {
        let due = (t.updated_at + RETENTION_MS).saturating_sub(now).max(1000);
        if soonest == 0 || due < soonest {
            soonest = due;
        }
    }
    soonest
}

pub fn next_action(status: Status) -> &'static str {
    match status {
        Status::Pending => {
            "The task is waiting. Ask the user to send the worker its check message, then call get_result again."
        }
        Status::Claimed => {
            "A worker has claimed the task. Call get_result again with wait_seconds 25."
        }
        Status::Done => "The result is ready. Review the code, then apply it.",
        Status::Failed => {
            "The task failed after 3 attempts. Resubmit it with a clearer or smaller specification."
        }
        Status::Cancelled => "The task was cancelled.",
    }
}

pub fn result_view(task: &Task, body: &Body) -> Value {
    json!({
        "task_id": task.id,
        "status": task.status.as_str(),
        "code": body.code,
        "files": body.result_files,
        "summary": body.summary,
        "attempts": task.attempts,
        "next_action": next_action(task.status),
    })
}

pub fn list_view(tasks: &[Task], status: &str, limit: usize, now: u64) -> Result<Value, String> {
    let wanted = if status.trim().is_empty() {
        Status::Pending
    } else {
        Status::parse(status.trim())?
    };
    let filter_on = !status.trim().is_empty();
    let mut rows: Vec<&Task> = tasks
        .iter()
        .filter(|t| !filter_on || t.status == wanted)
        .collect();
    rows.sort_by_key(|task| std::cmp::Reverse(task.created_at));
    let cap = limit.clamp(1, MAX_LIST_LIMIT);
    let list: Vec<Value> = rows
        .into_iter()
        .take(cap)
        .map(|t| {
            json!({
                "task_id": t.id,
                "title": t.title,
                "status": t.status.as_str(),
                "age_seconds": now.saturating_sub(t.created_at) / 1000,
            })
        })
        .collect();
    Ok(json!(list))
}

pub fn status_summary(tasks: &[Task], now: u64) -> Value {
    let count = |s: Status| tasks.iter().filter(|t| t.status == s).count();
    let oldest_pending = tasks
        .iter()
        .filter(|t| t.status == Status::Pending)
        .map(|t| t.created_at)
        .min()
        .map(|c| now.saturating_sub(c) / 1000)
        .unwrap_or(0);
    json!({
        "total": tasks.len(),
        "pending": count(Status::Pending),
        "claimed": count(Status::Claimed),
        "done": count(Status::Done),
        "failed": count(Status::Failed),
        "cancelled": count(Status::Cancelled),
        "oldest_pending_age_seconds": oldest_pending,
    })
}

/// Eight lowercase base32 characters from five random bytes.
pub fn id_from_bytes(bytes: &[u8; 5]) -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut acc: u64 = 0;
    for b in bytes {
        acc = (acc << 8) | (*b as u64);
    }
    (0..8usize)
        .map(|i| ALPHABET[((acc >> (35 - 5 * i)) & 31) as usize] as char)
        .collect()
}

/// Fixed window counter, one per role, kept in memory.
#[derive(Clone, Copy, Debug)]
pub struct RateWindow {
    pub start: u64,
    pub count: u32,
}

impl RateWindow {
    pub fn new() -> Self {
        RateWindow { start: 0, count: 0 }
    }

    pub fn allow(&mut self, now: u64) -> bool {
        if now.saturating_sub(self.start) >= 60_000 {
            self.start = now;
            self.count = 0;
        }
        if self.count >= RATE_LIMIT_PER_MINUTE {
            return false;
        }
        self.count += 1;
        true
    }
}

impl Default for RateWindow {
    fn default() -> Self {
        RateWindow::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(id: &str, target: &str, now: u64) -> (Task, Body) {
        new_task(id.to_string(), "Title", "Spec", "", target, now).unwrap()
    }

    #[test]
    fn creates_pending_task() {
        let (t, b) = make("aaaaaaaa", "", 10);
        assert_eq!(t.status, Status::Pending);
        assert_eq!(b.requirements, "Spec");
    }

    #[test]
    fn rejects_blank_and_oversized_fields() {
        assert!(new_task("a".into(), " ", "x", "", "", 0).is_err());
        assert!(new_task("a".into(), "t", " ", "", "", 0).is_err());
        let long = "x".repeat(MAX_REQUIREMENTS + 1);
        assert!(new_task("a".into(), "t", &long, "", "", 0).is_err());
    }

    #[test]
    fn picks_oldest_matching_task() {
        let mut tasks = vec![
            make("aaaaaaaa", "gemini", 1).0,
            make("bbbbbbbb", "", 2).0,
            make("cccccccc", "", 3).0,
        ];
        assert_eq!(pick_next(&tasks, "").unwrap(), 1);
        assert_eq!(pick_next(&tasks, "gemini").unwrap(), 0);
        claim(&mut tasks[0], 5, LEASE_MS);
        assert_eq!(pick_next(&tasks, "gemini").unwrap(), 1);
    }

    #[test]
    fn empty_queue_reports_message() {
        let tasks: Vec<Task> = Vec::new();
        assert_eq!(pick_next(&tasks, "").unwrap_err(), QUEUE_EMPTY);
    }

    #[test]
    fn complete_moves_to_done_and_refuses_repeat() {
        let (mut t, mut b) = make("aaaaaaaa", "", 1);
        claim(&mut t, 2, LEASE_MS);
        complete(&mut t, &mut b, "fn main() {}", "built", 3).unwrap();
        assert_eq!(t.status, Status::Done);
        assert_eq!(b.code, "fn main() {}");
        assert!(complete(&mut t, &mut b, "again", "", 4).is_err());
    }

    #[test]
    fn late_result_is_accepted_while_pending() {
        let (mut t, mut b) = make("aaaaaaaa", "", 1);
        assert!(complete(&mut t, &mut b, "code", "", 2).is_ok());
    }

    #[test]
    fn cancel_works_once() {
        let (mut t, _) = make("aaaaaaaa", "", 1);
        assert!(cancel(&mut t, 2).is_ok());
        assert!(cancel(&mut t, 3).is_err());
    }

    #[test]
    fn leases_expire_then_fail_after_three_attempts() {
        let mut tasks = vec![make("aaaaaaaa", "", 0).0];
        let mut now = 0;
        for round in 1..=MAX_ATTEMPTS {
            claim(&mut tasks[0], now, 1000);
            now += 1000;
            let changed = expire_claims(&mut tasks, now);
            assert_eq!(changed.len(), 1);
            if round < MAX_ATTEMPTS {
                assert_eq!(tasks[0].status, Status::Pending);
            }
        }
        assert_eq!(tasks[0].status, Status::Failed);
        assert_eq!(tasks[0].attempts, MAX_ATTEMPTS);
    }

    #[test]
    fn purge_removes_only_stale_finished_tasks() {
        let mut done = make("aaaaaaaa", "", 0).0;
        done.status = Status::Done;
        done.updated_at = 0;
        let pending = make("bbbbbbbb", "", 0).0;
        let mut tasks = vec![done, pending];
        assert!(purge(&mut tasks, RETENTION_MS - 1).is_empty());
        let removed = purge(&mut tasks, RETENTION_MS);
        assert_eq!(removed, vec!["aaaaaaaa".to_string()]);
        assert_eq!(tasks.len(), 1);
    }

    #[test]
    fn alarm_schedule_follows_state() {
        let mut tasks = vec![make("aaaaaaaa", "", 0).0];
        assert_eq!(next_alarm_ms(&tasks, 0, 60_000), 0);
        claim(&mut tasks[0], 0, LEASE_MS);
        assert_eq!(next_alarm_ms(&tasks, 0, 60_000), 60_000);
        tasks[0].status = Status::Done;
        tasks[0].updated_at = 0;
        assert_eq!(next_alarm_ms(&tasks, 0, 60_000), RETENTION_MS);
    }

    #[test]
    fn list_view_filters_and_sorts() {
        let mut a = make("aaaaaaaa", "", 1000).0;
        a.status = Status::Done;
        let b = make("bbbbbbbb", "", 2000).0;
        let tasks = vec![a, b];
        let all = list_view(&tasks, "", 20, 5000).unwrap();
        assert_eq!(all.as_array().unwrap().len(), 2);
        assert_eq!(all[0]["task_id"], "bbbbbbbb");
        let done = list_view(&tasks, "done", 20, 5000).unwrap();
        assert_eq!(done.as_array().unwrap().len(), 1);
        assert!(list_view(&tasks, "bogus", 20, 5000).is_err());
    }

    #[test]
    fn ids_are_eight_base32_characters() {
        let id = id_from_bytes(&[0xff, 0x00, 0xab, 0x12, 0x99]);
        assert_eq!(id.len(), 8);
        assert!(id
            .chars()
            .all(|c| c.is_ascii_lowercase() || ('2'..='7').contains(&c)));
        assert_eq!(id_from_bytes(&[0, 0, 0, 0, 0]), "aaaaaaaa");
    }

    #[test]
    fn rate_window_blocks_after_limit_and_resets() {
        let mut w = RateWindow::new();
        for _ in 0..RATE_LIMIT_PER_MINUTE {
            assert!(w.allow(1_000_000));
        }
        assert!(!w.allow(1_000_001));
        assert!(w.allow(1_000_000 + 60_000));
    }

    #[test]
    fn status_summary_counts() {
        let tasks = vec![make("aaaaaaaa", "", 0).0];
        let s = status_summary(&tasks, 5000);
        assert_eq!(s["pending"], 1);
        assert_eq!(s["oldest_pending_age_seconds"], 5);
    }
}
