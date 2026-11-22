use futures::StreamExt;
use worker::wasm_bindgen::JsValue;
use worker::*;

use crate::auth::{key_matches, route, Route};
use crate::model::{Role, MAX_BODY};

fn secret(env: &Env, name: &str) -> String {
    match env.secret(name) {
        Ok(value) => value.to_string(),
        _ => String::new(),
    }
}

async fn forward(env: &Env, path: &str, post: bool, body: String) -> Result<Response> {
    let namespace = env.durable_object("RELAY")?;
    let stub = namespace.id_from_name("main")?.get_stub()?;
    let mut init = RequestInit::new();
    if post {
        init.with_method(Method::Post);
        init.with_body(Some(JsValue::from_str(&body)));
    } else {
        init.with_method(Method::Get);
    }
    let request = Request::new_with_init(&format!("https://relay.internal{path}"), &init)?;
    stub.fetch_with_request(request).await
}

pub async fn handle(mut req: Request, env: Env) -> Result<Response> {
    let path = req.path();
    let is_post = req.method() == Method::Post;
    let orch = secret(&env, "ORCH_KEY");
    let work = secret(&env, "WORK_KEY");
    let bearer = || -> Result<String> {
        Ok(req
            .headers()
            .get("Authorization")?
            .and_then(|h| h.strip_prefix("Bearer ").map(str::to_owned))
            .unwrap_or_default())
    };
    let endpoint = match path.as_str() {
        "/orch/mcp" => Route::Mcp {
            role: Role::Orchestrator,
            key: bearer()?,
        },
        "/work/mcp" => Route::Mcp {
            role: Role::Worker,
            key: bearer()?,
        },
        "/status" => Route::Status { key: bearer()? },
        _ => route(&path),
    };
    match endpoint {
        Route::Health if req.method() == Method::Get => Response::ok("ok"),
        Route::Health => Response::error("Use GET", 405),
        Route::Mcp { role, key } => {
            let expected = match role {
                Role::Orchestrator => &orch,
                Role::Worker => &work,
            };
            if orch == work || !key_matches(expected, &key) {
                return Response::error("Unknown path", 404);
            }
            if !is_post {
                let mut reply = Response::empty()?.with_status(405);
                reply.headers_mut().set("Allow", "POST")?;
                return Ok(reply);
            }
            if let Some(origin) = req.headers().get("Origin")? {
                let trusted = env
                    .var("ALLOWED_ORIGINS")
                    .ok()
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                if origin != req.url()?.origin().ascii_serialization()
                    && !trusted
                        .split(',')
                        .map(str::trim)
                        .any(|s| !s.is_empty() && s == origin)
                {
                    return Response::error("Untrusted origin", 403);
                }
            }
            if let Some(version) = req.headers().get("MCP-Protocol-Version")? {
                if !crate::mcp::SUPPORTED_VERSIONS.contains(&version.as_str()) {
                    return Response::error("Unsupported MCP protocol version", 400);
                }
            }
            if req
                .headers()
                .get("Content-Length")?
                .and_then(|s| s.parse::<u64>().ok())
                .is_some_and(|n| n > MAX_BODY as u64)
            {
                return Response::error("Request exceeds 1000000 bytes", 413);
            }
            let mut bytes = Vec::new();
            if let Ok(mut stream) = req.stream() {
                while let Some(chunk) = stream.next().await {
                    let chunk = chunk?;
                    if chunk.len() > MAX_BODY - bytes.len() {
                        return Response::error("Request exceeds 1000000 bytes", 413);
                    }
                    bytes.extend_from_slice(&chunk);
                }
            }
            let body = match String::from_utf8(bytes) {
                Ok(text) => text,
                Err(_) => {
                    return Ok(Response::from_json(&crate::mcp::err(
                        &serde_json::Value::Null,
                        crate::mcp::PARSE_ERROR,
                        "Invalid UTF-8",
                    ))?
                    .with_status(400))
                }
            };
            let target = match role {
                Role::Orchestrator => "/rpc/orch",
                Role::Worker => "/rpc/work",
            };
            forward(&env, target, true, body).await
        }
        Route::Status { key } => {
            if orch == work || !key_matches(&orch, &key) {
                return Response::error("Unknown path", 404);
            }
            if req.method() != Method::Get {
                return Response::error("Use GET", 405);
            }
            forward(&env, "/status", false, String::new()).await
        }
        Route::Other => Response::error("Unknown path", 404),
    }
}
