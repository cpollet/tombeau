use crate::agent::{ErrorResponse, SetPasswordRequest, SetSecretRequest};

use crate::git::Repository;
use crate::shrine::{Shrine, ShrinePassword};
use crate::{Error, SHRINE_FILENAME};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, put};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use hyper::Server;
use hyperlocal::UnixServerExt;
use std::collections::HashMap;
use std::fs::remove_file;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::{mem, process};
use tokio::signal::ctrl_c;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::oneshot::{channel, Receiver, Sender};
use tokio_cron_scheduler::{Job, JobScheduler};
use tracing::log::{error, info};
use tracing::Level;
use tracing_subscriber::filter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use uuid::Uuid;

pub async fn serve(pidfile: String, socketfile: String) {
    let filter = filter::Targets::new()
        .with_target("tower_http::trace::on_response", Level::DEBUG)
        .with_target("tower_http::trace::on_request", Level::INFO)
        .with_target("tower_http::trace::make_span", Level::TRACE)
        .with_default(Level::INFO);
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(filter)
        .init();

    let (tx, rx) = channel::<()>();
    let state = AgentState::new(tx);

    let mut scheduler = JobScheduler::new().await.unwrap();

    {
        let state = state.clone();
        scheduler
            .add(
                Job::new_repeated(Duration::from_secs(1), move |_uuid, _l| {
                    state.clean_expired_passwords();
                })
                .unwrap(),
            )
            .await
            .unwrap();
    }

    scheduler.set_shutdown_handler(Box::new(|| {
        Box::pin(async move {
            info!("Shut down scheduler");
        })
    }));

    scheduler.start().await.unwrap();

    let app = Router::new()
        .route("/", delete(delete_agent))
        .route("/pid", get(get_pid))
        .route("/passwords", put(set_password))
        .route("/passwords", delete(delete_passwords))
        .route("/keys/:file/:key", get(get_key))
        .route("/keys/:file/:key", put(set_key))
        .with_state(state);

    if let Ok(x) = Server::bind_unix(&socketfile) {
        x.serve(app.into_make_service())
            .with_graceful_shutdown(shutdown(rx))
            .await
            .unwrap();

        remove_file(pidfile).unwrap();
        remove_file(socketfile).unwrap();
    } else {
        error!("Could not open socket.")
    }

    scheduler.shutdown().await.unwrap();
}

async fn shutdown(shutdown_http_signal_rx: Receiver<()>) {
    let ctrl_c = async {
        ctrl_c().await.expect("failed to install Ctrl+C handler");
    };

    let terminate = async {
        signal(SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    let http_shutdown = async {
        shutdown_http_signal_rx.await.ok();
    };

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
        _ = http_shutdown => {}
    }

    info!("Shut down HTTP server");
}

async fn delete_agent(State(state): State<AgentState>) {
    info!("delete_agent");
    let channel = channel::<()>();

    let mut sig = state.http_shutdown_tx.lock().unwrap();

    let _ = mem::replace(&mut *sig, channel.0).send(());
}

async fn get_pid() -> String {
    info!("get_pid");
    serde_json::to_string(&process::id()).unwrap()
}

async fn set_password(
    State(state): State<AgentState>,
    Json(set_password_request): Json<SetPasswordRequest>,
) {
    info!("set_password");
    state.set_password(set_password_request.uuid, set_password_request.password);
}

async fn delete_passwords(State(state): State<AgentState>) {
    info!("delete_passwords");
    state.delete_passwords();
}

async fn get_key(
    State(state): State<AgentState>,
    Path((path, key)): Path<(String, String)>,
) -> Response {
    info!("get_key `{}` from file `{}/{}`", key, path, SHRINE_FILENAME);

    let shrine = match open_shrine(state, &path) {
        Ok((shrine, _)) => shrine,
        Err(response) => return response,
    };

    match shrine.get(&key) {
        Err(_) => ErrorResponse::KeyNotFound { file: path, key }.into(),
        Ok(secret) => Json(secret).into_response(),
    }
}

fn open_shrine(state: AgentState, path: &str) -> Result<(Shrine, ShrinePassword), Response> {
    let shrine = match Shrine::from_path(PathBuf::from_str(path).unwrap()) {
        Err(Error::FileNotFound(_)) => {
            return Err(ErrorResponse::FileNotFound(path.to_string()).into())
        }
        Err(Error::IoRead(_)) => return Err(ErrorResponse::Read(path.to_string()).into()),
        Err(_) => return Err(ErrorResponse::Io(path.to_string()).into()),
        Ok(shrine) => shrine,
    };

    let uuid = shrine.uuid();

    let shrine_password = if shrine.requires_password() {
        match state.get_password(uuid) {
            None => return Err(ErrorResponse::Unauthorized(uuid).into()),
            Some(p) => p,
        }
    } else {
        ShrinePassword::from("")
    };

    let shrine = match shrine.open(&shrine_password) {
        Err(_) => return Err(ErrorResponse::Forbidden(uuid).into()),
        Ok(shrine) => shrine,
    };

    Ok((shrine, shrine_password))
}

async fn set_key(
    State(state): State<AgentState>,
    Path((path, key)): Path<(String, String)>,
    Json(request): Json<SetSecretRequest>,
) -> Response {
    info!("set_key `{}` on file `{}/{}`", key, path, SHRINE_FILENAME);

    let (mut shrine, shrine_password) = match open_shrine(state, &path) {
        Ok(v) => v,
        Err(response) => return response,
    };

    let repository = Repository::new(PathBuf::from_str(&path).unwrap(), &shrine);

    match shrine.set(&key, request.secret, request.mode) {
        Ok(_) => {}
        Err(Error::KeyNotFound(key)) => {
            return ErrorResponse::KeyNotFound { file: path, key }.into()
        }
        Err(_) => return ErrorResponse::Write(path).into(),
    }

    let shrine = match shrine.close(&shrine_password) {
        Ok(shrine) => shrine,
        Err(_) => return ErrorResponse::Write(path).into(),
    };
    if shrine.to_path(&path).is_err() {
        return ErrorResponse::Write(path).into();
    }

    if let Some(repository) = repository {
        if repository.commit_auto()
            && repository
                .open()
                .and_then(|r| r.create_commit("Update shrine"))
                .is_err()
        {
            return ErrorResponse::Write(path).into();
        }
    }

    Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body(Default::default())
        .unwrap()
}

#[derive(Clone)]
struct AgentState {
    http_shutdown_tx: Arc<Mutex<Sender<()>>>,
    passwords: Arc<Mutex<HashMap<Uuid, ATimePassword>>>,
}
type ATimePassword = (DateTime<Utc>, ShrinePassword);

impl AgentState {
    fn new(http_shutdown_tx: Sender<()>) -> Self {
        Self {
            http_shutdown_tx: Arc::new(Mutex::new(http_shutdown_tx)),
            passwords: Arc::new(Mutex::new(Default::default())),
        }
    }

    fn set_password(&self, uuid: Uuid, password: ShrinePassword) {
        self.passwords
            .lock()
            .unwrap()
            .insert(uuid, (Utc::now(), password));
    }

    fn delete_passwords(&self) {
        self.passwords.lock().unwrap().clear();
    }

    fn get_password(&self, uuid: Uuid) -> Option<ShrinePassword> {
        let mut passwords = self.passwords.lock().unwrap();
        match passwords.remove(&uuid) {
            None => None,
            Some((_, password)) => {
                passwords.insert(uuid, (Utc::now(), password.clone()));
                Some(password)
            }
        }
    }

    fn clean_expired_passwords(&self) {
        let lowest_barrier = Utc::now() - chrono::Duration::minutes(15);
        self.passwords
            .lock()
            .unwrap()
            .retain(|_, (atime, _)| (*atime).gt(&lowest_barrier));
    }
}
