//! User-facing job commands. CLI invocations are clients, never job supervisors.
use super::{CliError, ServerInfo, Transport};
use clap::{Args, Subcommand};
use serde_json::{json, Value};
use splitty_core::ids::JobId;

#[derive(Args, Clone)]
pub struct Common {
    #[arg(long, global = true)]
    addr: Option<String>,
    #[arg(long, global = true)]
    token: Option<String>,
    #[arg(long, global = true)]
    json: bool,
}
#[derive(Subcommand)]
pub enum Commands {
    /// Run a server-owned program in a background terminal.
    Run {
        #[command(flatten)]
        common: Common,
        #[arg(long, group = "placement")]
        tab: bool,
        #[arg(long,group="placement",value_parser=["left","right","above","below"])]
        split: Option<String>,
        #[arg(long, group = "placement")]
        new_screen: bool,
        #[arg(long)]
        background: bool,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        cwd: Option<std::path::PathBuf>,
        #[arg(long)]
        screen: Option<String>,
        #[arg(long)]
        pane: Option<String>,
        #[arg(long)]
        request_id: Option<String>,
        #[arg(last=true,required=true,num_args=1..)]
        argv: Vec<String>,
    },
    /// List visible managed jobs.
    Jobs {
        #[command(flatten)]
        common: Common,
    },
    Inspect {
        #[command(flatten)]
        common: Common,
        id: String,
    },
    Logs {
        #[command(flatten)]
        common: Common,
        id: String,
        #[arg(long, conflicts_with = "since")]
        tail: Option<usize>,
        #[arg(long)]
        since: Option<String>,
        #[arg(long, default_value = "65536")]
        limit: usize,
        #[arg(long,default_value="0s",value_parser=wait_ms)]
        wait: u64,
        #[arg(long,default_value="text",value_parser=["text","raw"])]
        format: String,
    },
    /// Send exact UTF-8 stdin (at most 16 KiB) to a job.
    Input {
        #[command(flatten)]
        common: Common,
        id: String,
    },
    Stop {
        #[command(flatten)]
        common: Common,
        id: String,
    },
    Forget {
        #[command(flatten)]
        common: Common,
        id: String,
    },
    /// Provision/revoke local agent grants or launch a client under one.
    Agent {
        #[command(flatten)]
        common: Common,
        #[command(subcommand)]
        action: AgentCommand,
    },
}
#[derive(Subcommand)]
pub enum AgentCommand {
    /// Print the built-in agent workflow guide (offline; no server required).
    Guide,
    Create {
        #[arg(long)]
        name: String,
        #[arg(long)]
        screen: Option<String>,
        #[arg(long)]
        pane: Option<String>,
    },
    List,
    Revoke {
        id: String,
    },
    Assign {
        id: String,
        #[arg(long)]
        job: String,
    },
    Reclaim {
        #[arg(long)]
        job: String,
    },
    Exec {
        id: String,
        #[arg(last=true,required=true,num_args=1..)]
        argv: Vec<String>,
    },
}
fn wait_ms(value: &str) -> Result<u64, String> {
    let (number, factor) = if let Some(n) = value.strip_suffix("ms") {
        (n, 1)
    } else {
        (value.strip_suffix('s').unwrap_or(value), 1000)
    };
    number
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(factor))
        .filter(|n| *n <= 30000)
        .ok_or_else(|| "wait must be 0..30s (or milliseconds with ms)".into())
}
fn usage(error: impl ToString) -> Failure {
    Failure {
        value: json!({"code":"usage","error":error.to_string()}),
        exit: 2,
    }
}
#[derive(Debug)]
struct Failure {
    value: Value,
    exit: i32,
}
impl From<CliError> for Failure {
    fn from(error: CliError) -> Self {
        let exit = if matches!(error, CliError::Usage(_)) {
            2
        } else {
            1
        };
        Self {
            value: json!({"code":if exit==2{"usage"}else{"request_failed"},"error":error.to_string()}),
            exit,
        }
    }
}
async fn request(
    server: &ServerInfo,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Result<Value, Failure> {
    let (status, mut value) = super::request(server, method, path, body).await?;
    if !(200..300).contains(&status) {
        value["status"] = json!(status);
        if value.get("code").is_none() {
            value["code"] = json!("api_error");
        }
        return Err(Failure { value, exit: 1 });
    }
    Ok(value)
}
fn path(id: &str, suffix: &str) -> Result<String, Failure> {
    if id.is_empty()
        || id.len() > 256
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(usage("invalid job or grant ID"));
    }
    Ok(format!("/{id}{suffix}"))
}
fn anchor(screen: Option<String>, pane: Option<String>) -> Result<Option<Value>, Failure> {
    match (screen, pane) {
        (Some(s), Some(p)) => Ok(Some(json!({"screenId":s,"paneId":p}))),
        (None, None) => Ok(None),
        _ => Err(usage("--screen and --pane must be supplied together")),
    }
}
impl Commands {
    fn common(&self) -> &Common {
        match self {
            Self::Run { common, .. }
            | Self::Jobs { common }
            | Self::Inspect { common, .. }
            | Self::Logs { common, .. }
            | Self::Input { common, .. }
            | Self::Stop { common, .. }
            | Self::Forget { common, .. }
            | Self::Agent { common, .. } => common,
        }
    }
    pub fn is_admin(&self) -> bool {
        matches!(self, Self::Agent { action, .. } if !matches!(action, AgentCommand::Guide))
    }
}
pub async fn run(command: Commands) -> i32 {
    let json_output = command.common().json;
    match execute(command).await {
        Ok((value, exit, text)) => {
            if !value.is_null() {
                if json_output {
                    println!("{value}");
                } else if let Some(text) = text {
                    print!("{text}");
                } else {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&value).expect("JSON value")
                    );
                }
            }
            exit
        }
        Err(failure) => {
            if json_output {
                println!("{}", failure.value);
            } else {
                eprintln!(
                    "Error: {}",
                    failure.value["error"].as_str().unwrap_or("command failed")
                );
                if let Some(id) = failure.value["requestId"].as_str() {
                    eprintln!("Request ID: {id} (retry this ID if the launch outcome is unknown)");
                }
            }
            failure.exit
        }
    }
}
async fn execute(command: Commands) -> Result<(Value, i32, Option<String>), Failure> {
    if matches!(
        command,
        Commands::Agent {
            action: AgentCommand::Guide,
            ..
        }
    ) {
        let text = super::agent_guide::text();
        return Ok((
            json!({"version":env!("CARGO_PKG_VERSION"),"guide":text}),
            0,
            Some(text),
        ));
    }
    let common = command.common().clone();
    if let Commands::Logs { format, tail, .. } = &command {
        if format == "raw" && (!common.json || tail.is_some()) {
            return Err(usage("raw logs require --json and cannot use --tail"));
        }
    }
    if let Commands::Run {
        tab,
        split,
        screen,
        pane,
        ..
    } = &command
    {
        if !tab && split.is_none() && (screen.is_some() || pane.is_some()) {
            return Err(usage("--screen/--pane require tab or split placement"));
        }
    }
    let server = super::discover(common.addr.as_deref(), common.token.as_deref())?;
    execute_at(command, server).await
}

async fn execute_at(
    command: Commands,
    server: ServerInfo,
) -> Result<(Value, i32, Option<String>), Failure> {
    let common = command.common().clone();
    if command.is_admin()
        && (server.source == "agent" || matches!(server.transport, Transport::Tcp { .. }))
    {
        return Err(usage(
            "agent administration requires the owner control socket",
        ));
    }
    if !command.is_admin() {
        let capabilities=request(&server,"GET","/api/jobs",None).await.map_err(|mut e|{if e.value["status"]==404{e.value=json!({"code":"unsupported","error":"server does not support managed jobs; update Splitty"});}e})?;
        if !capabilities["capabilities"]
            .as_array()
            .is_some_and(|a| a.iter().any(|c| c == "managed-jobs-v1"))
        {
            return Err(Failure {
                value: json!({"code":"unsupported","error":"server does not support managed jobs"}),
                exit: 1,
            });
        }
        if matches!(command, Commands::Jobs { .. }) {
            return Ok((capabilities, 0, None));
        }
    }
    let value = match command {
        Commands::Run {
            tab,
            split,
            name,
            cwd,
            screen,
            pane,
            request_id,
            argv,
            ..
        } => {
            let id = request_id.unwrap_or_else(|| JobId::new().to_string());
            let cwd = cwd.unwrap_or(std::env::current_dir().map_err(usage)?);
            let cwd = if cwd.is_absolute() {
                cwd
            } else {
                std::env::current_dir().map_err(usage)?.join(cwd)
            };
            let mut placement = if tab {
                json!({"kind":"tab"})
            } else if let Some(direction) = split {
                json!({"kind":"split","direction":direction})
            } else {
                json!({"kind":"screen"})
            };
            if placement["kind"] != "screen" {
                let (screen, pane) = if screen.is_some() || pane.is_some() {
                    (screen, pane)
                } else if server.source == "agent" {
                    (None, None)
                } else {
                    (
                        std::env::var("SPLITTY_SCREEN_ID").ok(),
                        std::env::var("SPLITTY_PANE_ID").ok(),
                    )
                };
                if let Some(anchor) = anchor(screen, pane)? {
                    placement["screenId"] = anchor["screenId"].clone();
                    placement["paneId"] = anchor["paneId"].clone();
                } else if server.source != "agent" {
                    return Err(usage("tab/split requires a pane and screen"));
                }
            }
            let body = json!({"requestId":id,"argv":argv,"cwd":cwd,"name":name,"placement":placement,"background":true});
            request(&server, "POST", "/api/jobs", Some(body))
                .await
                .map_err(|mut e| {
                    e.value["requestId"] = json!(id);
                    if e.value["code"] == "request_failed" {
                        e.value["outcome"] = json!("unknown");
                    }
                    e
                })?
        }
        Commands::Inspect { id, .. } => {
            request(
                &server,
                "GET",
                &format!("/api/jobs{}", path(&id, "")?),
                None,
            )
            .await?
        }
        Commands::Logs {
            id,
            tail,
            since,
            limit,
            wait,
            format,
            ..
        } => {
            if format == "raw" && (!common.json || tail.is_some()) {
                return Err(usage("raw logs require --json and cannot use --tail"));
            }
            let mut url =
                reqwest::Url::parse(&format!("http://localhost/api/jobs{}", path(&id, "/logs")?))
                    .map_err(usage)?;
            {
                let mut params = url.query_pairs_mut();
                params
                    .append_pair("limit", &limit.to_string())
                    .append_pair("waitMs", &wait.to_string())
                    .append_pair("format", &format);
                if let Some(tail) = tail {
                    params.append_pair("tail", &tail.to_string());
                }
                if let Some(cursor) = since {
                    params.append_pair("cursor", &cursor);
                }
            }
            let value = request(
                &server,
                "GET",
                &format!("{}?{}", url.path(), url.query().unwrap_or("")),
                None,
            )
            .await?;
            if !common.json {
                return Ok((
                    value.clone(),
                    0,
                    Some(value["data"].as_str().unwrap_or("").to_owned()),
                ));
            }
            value
        }
        Commands::Input { id, .. } => {
            use tokio::io::AsyncReadExt;
            let mut bytes = Vec::new();
            tokio::io::stdin()
                .take(16385)
                .read_to_end(&mut bytes)
                .await
                .map_err(usage)?;
            let data = input_data(bytes)?;
            request(
                &server,
                "POST",
                &format!("/api/jobs{}", path(&id, "/input")?),
                Some(json!({"data":data})),
            )
            .await?
        }
        Commands::Stop { id, .. } => {
            request(
                &server,
                "POST",
                &format!("/api/jobs{}", path(&id, "/stop")?),
                Some(json!({})),
            )
            .await?
        }
        Commands::Forget { id, .. } => {
            request(
                &server,
                "POST",
                &format!("/api/jobs{}", path(&id, "/forget")?),
                Some(json!({})),
            )
            .await?
        }
        Commands::Agent { action, .. } => match action {
            AgentCommand::Guide => unreachable!("local guide is handled before server discovery"),
            AgentCommand::List => request(&server, "GET", "/api/agents", None).await?,
            AgentCommand::Create { name, screen, pane } => {
                let (screen, pane) = if screen.is_none() && pane.is_none() {
                    (
                        std::env::var("SPLITTY_SCREEN_ID").ok(),
                        std::env::var("SPLITTY_PANE_ID").ok(),
                    )
                } else {
                    (screen, pane)
                };
                request(
                    &server,
                    "POST",
                    "/api/agents",
                    Some(json!({"name":name,"anchor":anchor(screen,pane)?})),
                )
                .await?
            }
            AgentCommand::Revoke { id } => {
                request(
                    &server,
                    "POST",
                    &format!("/api/agents{}", path(&id, "/revoke")?),
                    Some(json!({})),
                )
                .await?
            }
            AgentCommand::Assign { id, job } => {
                request(
                    &server,
                    "POST",
                    &format!("/api/jobs{}", path(&job, "/owner")?),
                    Some(json!({"agentId":id})),
                )
                .await?
            }
            AgentCommand::Reclaim { job } => {
                request(
                    &server,
                    "POST",
                    &format!("/api/jobs{}", path(&job, "/owner")?),
                    Some(json!({"agentId":null})),
                )
                .await?
            }
            AgentCommand::Exec { id, argv } => {
                if common.json {
                    return Err(usage(
                        "agent exec forwards the client's terminal; --json is not supported",
                    ));
                }
                let grant = request(
                    &server,
                    "GET",
                    &format!("/api/agents{}", path(&id, "")?),
                    None,
                )
                .await?;
                let socket = grant["socket"]
                    .as_str()
                    .ok_or_else(|| usage("grant has no socket"))?;
                let directory = splitty_core::pty::cli_path::current_cli_dir();
                let mut child = agent_client(
                    &argv,
                    socket,
                    directory.as_deref(),
                    std::env::var_os("PATH").as_deref(),
                )?;
                let status = child.status().await.map_err(usage)?;
                return Ok((Value::Null, status.code().unwrap_or(1), None));
            }
        },
        Commands::Jobs { .. } => unreachable!("list handled with capabilities"),
    };
    Ok((value, 0, None))
}

fn input_data(bytes: Vec<u8>) -> Result<String, Failure> {
    if bytes.len() > 16384 {
        return Err(usage("input exceeds 16 KiB"));
    }
    String::from_utf8(bytes).map_err(usage)
}

fn agent_client(
    argv: &[String],
    socket: &str,
    directory: Option<&std::path::Path>,
    path: Option<&std::ffi::OsStr>,
) -> Result<tokio::process::Command, Failure> {
    let mut child = tokio::process::Command::new(&argv[0]);
    child.args(&argv[1..]);
    for variable in [
        "SPLITTY_SOCK",
        "SPLITTY_TOKEN",
        "SPLITTY_SERVER",
        "SPLITTY_ADDR",
        "SPLITTY_BOOTSTRAP",
        "SPLITTY_TICKET",
        "SPLITTY_URL",
        "SPLITTY_CLI_DIR",
    ] {
        child.env_remove(variable);
    }
    child.env("SPLITTY_AGENT_SOCK", socket);
    if let Some(directory) = directory {
        child
            .env(
                "PATH",
                splitty_core::pty::cli_path::prepend(directory, path).map_err(usage)?,
            )
            .env("SPLITTY_CLI_DIR", directory);
    }
    Ok(child)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: Commands,
    }

    #[test]
    fn agent_client_preserves_literal_argv_cwd_and_grant_without_inheriting_owner_authority() {
        use std::ffi::{OsStr, OsString};
        let argv = vec!["program".into(), "two words".into(), "$(literal)".into()];
        let directory = std::path::Path::new("/CLI with spaces ö");
        let previous =
            std::env::join_paths([std::path::Path::new("/one"), std::path::Path::new("/two")])
                .expect("PATH");
        let child = agent_client(
            &argv,
            "/selected-grant.sock",
            Some(directory),
            Some(&previous),
        )
        .expect("child");
        let command = child.as_std();
        assert_eq!(command.get_program(), OsStr::new("program"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![OsStr::new("two words"), OsStr::new("$(literal)")]
        );
        assert!(
            command.get_current_dir().is_none(),
            "inherit the caller's cwd"
        );
        let env: std::collections::HashMap<_, _> = command
            .get_envs()
            .map(|(key, value)| (key.to_os_string(), value.map(OsStr::to_os_string)))
            .collect();
        assert_eq!(
            env.get(OsStr::new("SPLITTY_AGENT_SOCK")),
            Some(&Some(OsString::from("/selected-grant.sock")))
        );
        assert_eq!(
            env.get(OsStr::new("SPLITTY_CLI_DIR")),
            Some(&Some(directory.as_os_str().to_os_string()))
        );
        for name in [
            "SPLITTY_SOCK",
            "SPLITTY_TOKEN",
            "SPLITTY_SERVER",
            "SPLITTY_ADDR",
            "SPLITTY_BOOTSTRAP",
            "SPLITTY_TICKET",
            "SPLITTY_URL",
        ] {
            assert_eq!(env.get(OsStr::new(name)), Some(&None), "removed {name}");
        }
        let path = env[OsStr::new("PATH")].as_ref().expect("PATH");
        assert_eq!(
            std::env::split_paths(path).collect::<Vec<_>>(),
            vec![
                directory,
                std::path::Path::new("/one"),
                std::path::Path::new("/two")
            ]
        );
    }

    #[tokio::test]
    async fn guide_parses_as_local_information_and_returns_one_versioned_document() {
        for args in [
            vec!["splitty", "agent", "guide"],
            vec!["splitty", "agent", "guide", "--json"],
        ] {
            let command = TestCli::try_parse_from(args).expect("guide").command;
            assert!(!command.is_admin());
            let (value, status, text) = execute(command).await.expect("offline guide");
            assert_eq!(status, 0);
            assert_eq!(value["version"], env!("CARGO_PKG_VERSION"));
            assert_eq!(value["guide"].as_str(), text.as_deref());
            assert_eq!(value.as_object().expect("object").len(), 2);
        }
        for args in [
            vec!["splitty", "agent", "list"],
            vec!["splitty", "agent", "create", "--name", "a"],
            vec!["splitty", "agent", "revoke", "id"],
            vec!["splitty", "agent", "assign", "id", "--job", "job"],
            vec!["splitty", "agent", "reclaim", "--job", "job"],
            vec!["splitty", "agent", "exec", "id", "--", "program"],
        ] {
            assert!(TestCli::try_parse_from(args)
                .expect("admin")
                .command
                .is_admin());
        }
    }

    #[test]
    fn guide_examples_parse_as_the_existing_command_surface() {
        // These are the command lines in the guide (stdin's printf is not CLI argv).
        for args in [
            vec!["splitty", "jobs", "--json"],
            vec![
                "splitty",
                "run",
                "--tab",
                "--background",
                "--name",
                "http-demo",
                "--json",
                "--",
                "python3",
                "-m",
                "http.server",
                "8000",
                "--bind",
                "127.0.0.1",
            ],
            vec!["splitty", "inspect", "JOB_ID", "--json"],
            vec!["splitty", "logs", "JOB_ID", "--tail", "50", "--json"],
            vec![
                "splitty",
                "logs",
                "JOB_ID",
                "--since",
                "NEXT_CURSOR",
                "--limit",
                "65536",
                "--wait",
                "5s",
                "--json",
            ],
            vec!["splitty", "input", "JOB_ID", "--json"],
            vec!["splitty", "stop", "JOB_ID", "--json"],
            vec!["splitty", "forget", "JOB_ID", "--json"],
        ] {
            assert!(TestCli::try_parse_from(args).is_ok());
        }
    }
    #[test]
    fn argv_is_literal_and_conflicting_placements_and_log_selections_are_usage_errors() {
        let parsed = TestCli::try_parse_from([
            "splitty",
            "run",
            "--tab",
            "--background",
            "--json",
            "--",
            "printf",
            "two words",
            "$(literal)",
        ])
        .expect("parse");
        match parsed.command {
            Commands::Run {
                argv, tab, common, ..
            } => {
                assert!(tab && common.json);
                assert_eq!(argv, ["printf", "two words", "$(literal)"]);
            }
            _ => panic!("run"),
        }
        assert!(TestCli::try_parse_from([
            "splitty", "run", "--tab", "--split", "right", "--", "program"
        ])
        .is_err());
        assert!(TestCli::try_parse_from([
            "splitty", "logs", "job", "--tail", "10", "--since", "cursor"
        ])
        .is_err());
        assert_eq!(wait_ms("5s").expect("seconds"), 5000);
        assert_eq!(wait_ms("20ms").expect("millis"), 20);
        assert!(wait_ms("31s").is_err());
        assert!(anchor(Some("screen".into()), None).is_err());
        assert!(path("../agents", "").is_err());
        assert_eq!(input_data(br"\n".to_vec()).ok().as_deref(), Some(r"\n"));
        assert!(input_data(vec![0xff]).is_err());
        assert!(input_data(vec![b'x'; 16385]).is_err());
        for direction in ["left", "right", "above", "below"] {
            assert!(
                matches!(TestCli::try_parse_from(["splitty","run","--split",direction,"--","program"]).expect("direction").command,Commands::Run{split:Some(ref d),..} if d==direction)
            );
        }
    }

    #[tokio::test]
    async fn old_servers_are_refused_and_uncertain_starts_report_the_original_request_key() {
        use axum::{routing::get, Json, Router};
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
        let capture = requests.clone();
        let router = Router::new().route(
            "/api/jobs",
            get(|| async { Json(json!({"jobs":[]})) }).post(
                move |Json(body): Json<Value>| async move {
                    capture.lock().expect("requests").push(body);
                    "not JSON"
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let addr = listener.local_addr().expect("address");
        let server = ServerInfo {
            transport: Transport::Tcp {
                addr: addr.to_string(),
                token: "test".into(),
            },
            source: "test",
        };
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.expect("serve");
        });
        let command = || {
            TestCli::try_parse_from(["splitty", "run", "--json", "--", "program", "two words"])
                .expect("run")
                .command
        };
        let old = execute_at(command(), server)
            .await
            .expect_err("unsupported");
        assert_eq!(old.value["code"], "unsupported");
        assert!(requests.lock().expect("requests").is_empty());
        task.abort();
        let capture = requests.clone();
        let router = Router::new().route(
            "/api/jobs",
            get(|| async { Json(json!({"capabilities":["managed-jobs-v1"],"jobs":[]})) }).post(
                move |Json(body): Json<Value>| async move {
                    capture.lock().expect("requests").push(body);
                    "not JSON"
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let addr = listener.local_addr().expect("address");
        let server = ServerInfo {
            transport: Transport::Tcp {
                addr: addr.to_string(),
                token: "test".into(),
            },
            source: "test",
        };
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.expect("serve");
        });
        let error = execute_at(command(), server).await.expect_err("uncertain");
        assert_eq!(error.value["outcome"], "unknown");
        let captured = requests.lock().expect("requests");
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0]["requestId"], error.value["requestId"]);
        assert_eq!(captured[0]["argv"], json!(["program", "two words"]));
        assert!(captured[0]["cwd"].as_str().expect("cwd").starts_with('/'));
        task.abort();
    }
}
