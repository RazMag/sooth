//! A machine reached over its agent's API (see [`super::api`]). Plain HTTP
//! with a bearer token for now; pairing with pinned keys over HTTPS or SSH
//! replaces the shared token in the next phase.
//!
//! Nothing outside tests builds one until the hub links machines.
#![cfg_attr(not(test), allow(dead_code))]

use std::sync::Arc;
use std::time::Duration;

use reqwest::{Method, StatusCode, Url};
use serde::de::DeserializeOwned;

use super::api::{ApiErrorBody, PREFIX};
use super::{Action, Facts, UnitRecord};
use crate::error::AppError;
use crate::journal::LogLine;
use crate::quadlet::{QuadletError, UnitKind};
use crate::systemd::UnitStatus;

#[derive(Clone)]
pub struct RemoteMachine {
    base: Url,
    token: Arc<str>,
    http: reqwest::Client,
}

impl RemoteMachine {
    /// `base` is the agent's address, e.g. `http://10.0.0.5:8421`.
    pub fn new(base: &str, token: &str) -> Result<Self, AppError> {
        let base = Url::parse(base)
            .map_err(|e| AppError::Remote(format!("bad agent address '{base}': {e}")))?;
        if base.cannot_be_a_base() {
            return Err(AppError::Remote(format!("bad agent address '{base}'")));
        }
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            // A stop can take as long as the container's stop timeout.
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| AppError::Remote(format!("couldn't set up the HTTP client: {e}")))?;
        Ok(Self {
            base,
            token: Arc::from(token),
            http,
        })
    }

    /// `PREFIX` + `segments`, each one percent-encoded as a single segment.
    fn url(&self, segments: &[&str]) -> Url {
        let mut url = self.base.clone();
        if let Ok(mut path) = url.path_segments_mut() {
            path.pop_if_empty();
            path.extend(PREFIX.trim_start_matches('/').split('/'));
            path.extend(segments);
        }
        url
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: Method,
        url: Url,
        subject: &str,
    ) -> Result<T, AppError> {
        let res = self
            .http
            .request(method, url)
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|e| AppError::Remote(format!("couldn't reach {}: {e}", self.base)))?;
        let status = res.status();
        if status.is_success() {
            return res
                .json()
                .await
                .map_err(|e| AppError::Remote(format!("unreadable answer from the agent: {e}")));
        }
        let message = res
            .json::<ApiErrorBody>()
            .await
            .map(|b| b.error)
            .unwrap_or_else(|_| status.to_string());
        Err(match status {
            StatusCode::NOT_FOUND => AppError::NotFound(subject.to_string()),
            StatusCode::UNPROCESSABLE_ENTITY => QuadletError::Validation(message).into(),
            StatusCode::UNAUTHORIZED => {
                AppError::Remote("the agent refused this hub's token".into())
            }
            _ => AppError::Remote(format!("the agent said: {message}")),
        })
    }

    pub async fn facts(&self) -> Result<Facts, AppError> {
        self.call(Method::GET, self.url(&["info"]), "info").await
    }

    pub async fn records(&self, kinds: &[UnitKind]) -> Result<Vec<UnitRecord>, AppError> {
        let mut url = self.url(&["units"]);
        let kinds: Vec<&str> = kinds.iter().map(|k| k.extension()).collect();
        url.query_pairs_mut().append_pair("kinds", &kinds.join(","));
        self.call(Method::GET, url, "units").await
    }

    pub async fn status(&self, file_name: &str) -> Result<UnitStatus, AppError> {
        let url = self.url(&["units", file_name, "status"]);
        self.call(Method::GET, url, file_name).await
    }

    pub async fn act(&self, file_name: &str, action: Action) -> Result<UnitStatus, AppError> {
        let url = self.url(&["units", file_name, action.as_str()]);
        self.call(Method::POST, url, file_name).await
    }

    pub async fn logs(&self, file_name: &str, lines: u32) -> Result<Vec<LogLine>, AppError> {
        let mut url = self.url(&["units", file_name, "logs"]);
        url.query_pairs_mut()
            .append_pair("lines", &lines.to_string());
        self.call(Method::GET, url, file_name).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::Machine;
    use crate::machine::api::{ApiState, router};
    use crate::machine::fake::FakeMachine;

    /// Serves the agent API over `fake` on a free local port and returns a
    /// remote machine pointed at it with `token`.
    async fn agent(fake: FakeMachine, token: &str) -> Machine {
        let app = router(ApiState {
            machine: Machine::Fake(fake),
            token: Arc::from("s3cret"),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Machine::Remote(RemoteMachine::new(&format!("http://{addr}"), token).unwrap())
    }

    fn fake() -> FakeMachine {
        FakeMachine::with_units(&[
            ("", "web.container", "[Container]\nImage=nginx\n"),
            ("media", "data.volume", "[Volume]\n"),
        ])
    }

    #[tokio::test]
    async fn a_remote_machine_lists_parses_and_drives_the_agents_units() {
        let fake = fake();
        let remote = agent(fake.clone(), "s3cret").await;

        assert_eq!(remote.facts().await.unwrap().hostname, "fake01");

        let (with_status, all) = remote.units(&[UnitKind::Container]).await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].rel_path(), "media/data.volume");
        assert_eq!(with_status.len(), 1);
        let (web, status) = &with_status[0];
        assert_eq!(
            web.section("Container").unwrap().get("Image"),
            Some("nginx")
        );
        assert!(!status.is_active());

        let after = remote.act("web.container", Action::Start).await.unwrap();
        assert!(after.is_active());
        assert!(remote.status("web.container").await.unwrap().is_active());
        assert_eq!(
            fake.actions(),
            vec![("web.container".to_string(), Action::Start)]
        );

        let logs = remote.logs("web.container", 50).await.unwrap();
        assert_eq!(logs[0].message, "hello from web.container");
    }

    #[tokio::test]
    async fn agent_errors_come_back_as_the_same_kind_of_error() {
        let remote = agent(fake(), "s3cret").await;
        assert!(matches!(
            remote.status("nope.container").await,
            Err(AppError::NotFound(f)) if f == "nope.container"
        ));

        let wrong = agent(fake(), "guess").await;
        assert!(matches!(
            wrong.facts().await,
            Err(AppError::Remote(m)) if m.contains("token")
        ));
    }

    #[test]
    fn urls_keep_the_base_path_and_encode_each_segment() {
        let m = RemoteMachine::new("http://h:1/agent/", "t").unwrap();
        assert_eq!(
            m.url(&["units", "a b.container", "status"]).as_str(),
            "http://h:1/agent/api/v1/units/a%20b.container/status"
        );
        assert!(RemoteMachine::new("not a url", "t").is_err());
    }
}
