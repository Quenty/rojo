use std::io::Read;
use std::sync::{Arc, Mutex};

use rouille;
use serde;
use serde_json;

use core::Config;
use project::Project;
use vfs::{Vfs, VfsChange};
use rbx::RbxItem;
use plugin::PluginChain;
use plugins::{ScriptPlugin, DefaultPlugin};

static MAX_BODY_SIZE: usize = 25 * 1024 * 1025; // 25 MiB

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ServerInfo<'a> {
    server_version: &'static str,
    protocol_version: u64,
    server_id: &'a str,
    project: &'a Project,
    current_time: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadResult<'a> {
    items: Vec<Option<RbxItem>>,
    server_id: &'a str,
    current_time: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChangesResult<'a> {
    changes: &'a [VfsChange],
    server_id: &'a str,
    current_time: f64,
}

fn json<T: serde::Serialize>(value: T) -> rouille::Response {
    let data = serde_json::to_string(&value).unwrap();
    rouille::Response::from_data("application/json", data)
}

fn read_json_text(request: &rouille::Request) -> Option<String> {
    match request.header("Content-Type") {
        Some(header) => if !header.starts_with("application/json") {
            return None;
        },
        None => return None,
    }

    let body = match request.data() {
        Some(v) => v,
        None => return None,
    };

    let mut out = Vec::new();
    match body.take(MAX_BODY_SIZE.saturating_add(1) as u64)
        .read_to_end(&mut out)
    {
        Ok(_) => {},
        Err(_) => return None,
    }

    if out.len() > MAX_BODY_SIZE {
        return None;
    }

    let parsed = match String::from_utf8(out) {
        Ok(v) => v,
        Err(_) => return None,
    };

    Some(parsed)
}

fn read_json<T>(request: &rouille::Request) -> Option<T>
where
    T: serde::de::DeserializeOwned,
{
    let body = match read_json_text(&request) {
        Some(v) => v,
        None => return None,
    };

    let parsed = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => return None,
    };

    Some(parsed)
}

pub fn start(config: Config, project: Project, vfs: Arc<Mutex<Vfs>>) {
    let address = format!("localhost:{}", config.port);

    let server_id = config.server_id.to_string();

    // Fine, rouille, I'll put things in a 'static Arc<Mutex>...
    lazy_static! {
        static ref PLUGIN_CHAIN: Arc<Mutex<PluginChain>> = Arc::new(Mutex::new(PluginChain::new(vec![
            Box::new(ScriptPlugin::new()),
            Box::new(DefaultPlugin::new()),
        ])));
    }

    rouille::start_server(address, move |request| {
        router!(request,
            (GET) (/) => {
                let current_time = {
                    let vfs = vfs.lock().unwrap();

                    vfs.current_time()
                };

                json(ServerInfo {
                    server_version: env!("CARGO_PKG_VERSION"),
                    protocol_version: 1,
                    server_id: &server_id,
                    project: &project,
                    current_time,
                })
            },

            (GET) (/changes/{ last_time: f64 }) => {
                let vfs = vfs.lock().unwrap();
                let current_time = vfs.current_time();
                let changes = vfs.changes_since(last_time);

                json(ChangesResult {
                    changes,
                    server_id: &server_id,
                    current_time,
                })
            },

            (POST) (/read) => {
                let plugin_chain = PLUGIN_CHAIN.lock().unwrap();

                let read_request: Vec<Vec<String>> = match read_json(&request) {
                    Some(v) => v,
                    None => return rouille::Response::empty_400(),
                };

                let (items, current_time) = {
                    let vfs = vfs.lock().unwrap();

                    let current_time = vfs.current_time();

                    let mut items = Vec::new();

                    for route in &read_request {
                        match vfs.read(&route) {
                            Ok(v) => items.push(Some(v)),
                            Err(_) => items.push(None),
                        }
                    }

                    (items, current_time)
                };

                let rbx_items = items
                    .iter()
                    .map(|item| {
                        match *item {
                            Some(ref item) => plugin_chain.transform_file(item),
                            None => None,
                        }
                    })
                    .collect::<Vec<_>>();

                json(ReadResult {
                    server_id: &server_id,
                    items: rbx_items,
                    current_time,
                })
            },

            (POST) (/write) => {
                let write_request: Vec<Vec<String>> = match read_json(&request) {
                    Some(v) => v,
                    None => return rouille::Response::empty_400(),
                };

                
            },

            _ => rouille::Response::empty_404()
        )
    });
}
