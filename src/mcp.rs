//! Model Context Protocol (MCP) server for MBHub.
//!
//! Provides a standard stdio JSON-RPC 2.0 interface for AI developer tools
//! (Cursor, Claude Desktop, Goose, Claude Code, and local agent frameworks).
//!
//! Exposes tools:
//! - `mbhub_ask`: Query collective memory (L1 SQLite -> L2 P2P Swarm -> L3 BYOK).
//! - `mbhub_status`: Inspect node status, peer connectivity, and local shard records.

use std::io::{self, BufRead, Write};
use serde_json::{json, Value};

use crate::db;
use crate::headless;
use crate::ipc::{self, IpcRequest, IpcResponse};
use crate::model::Settings;

/// Runs the standard stdio JSON-RPC 2.0 MCP server loop.
pub fn run_mcp_server(accept_terms: bool) -> io::Result<()> {
    if accept_terms {
        db::set_meta("terms_accepted", "true");
        eprintln!("[mcp] MBHub Terms of Service accepted via CLI flag.");
    }

    eprintln!("[mcp] MBHub Model Context Protocol (MCP) server active on stdio.");

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let reader = stdin.lock();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[mcp] Error reading line from stdin: {}", e);
                break;
            }
        };

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(resp) = handle_json_rpc(trimmed) {
            stdout.write_all(resp.as_bytes())?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
        }
    }

    eprintln!("[mcp] Stdio closed. Shutting down MCP server.");
    Ok(())
}

/// Parses a single JSON-RPC line and produces an optional response.
/// Returns `None` for notifications (no `id`).
pub fn handle_json_rpc(raw: &str) -> Option<String> {
    let val: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => {
            let err_resp = json!({
                "jsonrpc": "2.0",
                "id": Value::Null,
                "error": {
                    "code": -32700,
                    "message": format!("Parse error: {}", e)
                }
            });
            return Some(err_resp.to_string());
        }
    };

    let id = val.get("id").cloned();
    let method = val.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = val.get("params").cloned().unwrap_or(Value::Null);

    // If there is no `id` (or id is null and not an error response), this is a notification.
    if val.get("id").is_none() {
        // Notifications do not receive responses.
        if method == "notifications/initialized" {
            eprintln!("[mcp] Client initialized notification received.");
        }
        return None;
    }

    let id = id.unwrap_or(Value::Null);

    let response = match method {
        "initialize" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "mbhub",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        }),

        "ping" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {}
        }),

        "tools/list" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "tools": [
                    {
                        "name": "mbhub_ask",
                        "description": "Query MBHub's decentralized collective memory (L1 local SQLite, L2 P2P swarm, and L3 BYOK fallback) for instant, verified answers.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "query": {
                                    "type": "string",
                                    "description": "The question or technical inquiry to search or resolve in collective memory."
                                }
                            },
                            "required": ["query"]
                        }
                    },
                    {
                        "name": "mbhub_status",
                        "description": "Check the operational status of the MBHub node, including daemon status, peer swarm connectivity, and local storage allocation.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {}
                        }
                    }
                ]
            }
        }),

        "tools/call" => {
            let tool_name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));

            match tool_name {
                "mbhub_ask" => {
                    let query = args.get("query").and_then(|q| q.as_str()).unwrap_or("").trim();
                    if query.is_empty() {
                        json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "content": [
                                    {
                                        "type": "text",
                                        "text": "Error: Missing or empty 'query' argument."
                                    }
                                ],
                                "isError": true
                            }
                        })
                    } else if db::get_meta("terms_accepted") != Some("true".to_string()) {
                        json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "content": [
                                    {
                                        "type": "text",
                                        "text": "Error: MBHub Terms of Service have not been accepted yet. Please launch `mbhub` once in your terminal to review and accept the Terms of Service, or run `mbhub mcp --accept-terms`."
                                    }
                                ],
                                "isError": true
                            }
                        })
                    } else {
                        let (text, is_err) = execute_mcp_ask(query);
                        json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "content": [
                                    {
                                        "type": "text",
                                        "text": text
                                    }
                                ],
                                "isError": is_err
                            }
                        })
                    }
                }

                "mbhub_status" => {
                    let text = execute_mcp_status();
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [
                                {
                                    "type": "text",
                                    "text": text
                                }
                            ],
                            "isError": false
                        }
                    })
                }

                other => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "content": [
                            {
                                "type": "text",
                                "text": format!("Unknown tool: {}", other)
                            }
                        ],
                        "isError": true
                    }
                }),
            }
        }

        "resources/list" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "resources": []
            }
        }),

        "prompts/list" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "prompts": []
            }
        }),

        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32601,
                "message": format!("Method not found: {}", method)
            }
        }),
    };

    Some(response.to_string())
}

/// Executes an ask query for MCP by querying the daemon first, or falling back
/// to local headless execution.
fn execute_mcp_ask(query: &str) -> (String, bool) {
    // 1. Try communicating with background daemon via IPC
    if let Some(ipc_resp) = ipc::try_query_daemon(&IpcRequest::Ask {
        query: query.to_string(),
    }) {
        match ipc_resp {
            IpcResponse::Answer {
                question,
                content,
                source,
                similarity,
                ..
            } => {
                let text = format!(
                    "# {}\n\n{}\n\n---\nSource: {} (daemon IPC) | Hit Rate: {:.2}%",
                    question, content, source, similarity
                );
                (text, false)
            }
            IpcResponse::Error(err) => (format!("Error: {}", err), true),
            _ => ("Error: Unexpected response from MBHub daemon".to_string(), true),
        }
    } else {
        // 2. Standalone fallback: execute directly via 3-tier pipeline
        match headless::execute_ask(query, None) {
            Ok(IpcResponse::Answer {
                question,
                content,
                source,
                similarity,
                ..
            }) => {
                let text = format!(
                    "# {}\n\n{}\n\n---\nSource: {} | Hit Rate: {:.2}%",
                    question, content, source, similarity
                );
                (text, false)
            }
            Ok(IpcResponse::Error(err)) | Err(err) => (format!("Error: {}", err), true),
            _ => ("Error: Unexpected query response".to_string(), true),
        }
    }
}

/// Gathers node operational status for MCP.
fn execute_mcp_status() -> String {
    if let Some(IpcResponse::Status {
        running,
        peers,
        reserved_gb,
        records,
    }) = ipc::try_query_daemon(&IpcRequest::Status)
    {
        format!(
            "MBHub Node Status:\n- Daemon Status: {}\n- P2P Swarm Peers: {}\n- Local Shard Records: {}\n- Storage Quota: {} GB",
            if running { "Active (background IPC)" } else { "Inactive" },
            peers,
            records,
            reserved_gb
        )
    } else {
        let settings = Settings::load();
        let records = db::count_records();
        format!(
            "MBHub Node Status:\n- Daemon Status: Inactive (standalone mode)\n- P2P Swarm Peers: 0 (start daemon or TUI for active swarm)\n- Local Shard Records: {}\n- Storage Quota: {} GB",
            records,
            settings.reserved_gb
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_initialize() {
        let req = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05"
            }
        });
        let res_str = handle_json_rpc(&req.to_string()).expect("response expected");
        let res: Value = serde_json::from_str(&res_str).expect("valid json");

        assert_eq!(res["id"], 1);
        assert_eq!(res["result"]["serverInfo"]["name"], "mbhub");
        assert_eq!(res["result"]["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(res["result"]["protocolVersion"], "2024-11-05");
    }

    #[test]
    fn test_mcp_ping() {
        let req = json!({
            "jsonrpc": "2.0",
            "id": "ping-123",
            "method": "ping"
        });
        let res_str = handle_json_rpc(&req.to_string()).expect("response expected");
        let res: Value = serde_json::from_str(&res_str).expect("valid json");

        assert_eq!(res["id"], "ping-123");
        assert!(res["result"].is_object());
    }

    #[test]
    fn test_mcp_notifications_ignored() {
        let req = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        let res_str = handle_json_rpc(&req.to_string());
        assert!(res_str.is_none());
    }

    #[test]
    fn test_mcp_tools_list() {
        let req = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        });
        let res_str = handle_json_rpc(&req.to_string()).expect("response expected");
        let res: Value = serde_json::from_str(&res_str).expect("valid json");

        let tools = res["result"]["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 2);
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"mbhub_ask"));
        assert!(names.contains(&"mbhub_status"));
    }

    #[test]
    fn test_mcp_unknown_method() {
        let req = json!({
            "jsonrpc": "2.0",
            "id": 42,
            "method": "unknown_function"
        });
        let res_str = handle_json_rpc(&req.to_string()).expect("response expected");
        let res: Value = serde_json::from_str(&res_str).expect("valid json");

        assert_eq!(res["id"], 42);
        assert_eq!(res["error"]["code"], -32601);
    }

    #[test]
    fn test_mcp_status_call() {
        let req = json!({
            "jsonrpc": "2.0",
            "id": 10,
            "method": "tools/call",
            "params": {
                "name": "mbhub_status",
                "arguments": {}
            }
        });
        let res_str = handle_json_rpc(&req.to_string()).expect("response expected");
        let res: Value = serde_json::from_str(&res_str).expect("valid json");

        assert_eq!(res["id"], 10);
        assert_eq!(res["result"]["isError"], false);
        let text = res["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("MBHub Node Status:"));
    }

    #[test]
    fn test_mcp_ask_empty_query() {
        let req = json!({
            "jsonrpc": "2.0",
            "id": 11,
            "method": "tools/call",
            "params": {
                "name": "mbhub_ask",
                "arguments": {
                    "query": "   "
                }
            }
        });
        let res_str = handle_json_rpc(&req.to_string()).expect("response expected");
        let res: Value = serde_json::from_str(&res_str).expect("valid json");

        assert_eq!(res["id"], 11);
        assert_eq!(res["result"]["isError"], true);
    }
}
