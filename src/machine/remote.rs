//! A machine reached over its agent's API (see [`super::api`]): HTTPS with
//! mutual TLS, this hub presenting its own link certificate and accepting
//! only the agent certificate it pinned at pairing (`link::verify`).
//!
//! [`probe`] and [`pair`] are the two calls made before a machine is linked:
//! the first learns the agent's fingerprint for the person to confirm, the
//! second sends the pairing code over a connection pinned to it.

use std::sync::Arc;
use std::time::Duration;

use reqwest::{Method, StatusCode, Url};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::api::{ApiErrorBody, Hello, PREFIX, PairRequest, Paired};
use super::{Action, Facts, UnitRecord};
use crate::error::AppError;
use crate::journal::LogLine;
use crate::link::verify::PinnedServer;
use crate::link::{Fingerprint, Identity};
use crate::quadlet::{QuadletError, UnitKind};
use crate::systemd::UnitStatus;

#[derive(Clone)]
pub struct RemoteMachine {
    agent: Agent,
}

/// An HTTPS client for one agent address.
#[derive(Clone)]
struct Agent {
    base: Url,
    http: reqwest::Client,
}

/// `https://host:port` with nothing odd in it. Plain `http` is refused: the
/// pinning lives in TLS.
fn parse_base(base: &str) -> Result<Url, AppError> {
    let bad = |why: &str| AppError::Remote(format!("bad machine address '{base}': {why}"));
    let url = Url::parse(base.trim()).map_err(|e| bad(&e.to_string()))?;
    if url.scheme() != "https" {
        return Err(bad("it must start with https://"));
    }
    if url.cannot_be_a_base() || url.host_str().is_none() {
        return Err(bad("it needs a host"));
    }
    Ok(url)
}

impl Agent {
    fn new(base: &str, identity: &Identity, server: Arc<PinnedServer>) -> Result<Self, AppError> {
        let base = parse_base(base)?;
        let tls = identity
            .client_config(server)
            .map_err(|e| AppError::Remote(format!("couldn't set up TLS: {e}")))?;
        let http = reqwest::Client::builder()
            .tls_backend_preconfigured(tls)
            .connect_timeout(Duration::from_secs(5))
            // A stop can take as long as the container's stop timeout.
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| AppError::Remote(format!("couldn't set up the HTTP client: {e}")))?;
        Ok(Self { base, http })
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
        body: Option<&impl Serialize>,
        subject: &str,
    ) -> Result<T, AppError> {
        let mut req = self.http.request(method, url);
        if let Some(body) = body {
            req = req.json(body);
        }
        let res = req.send().await.map_err(|e| {
            AppError::Remote(format!("couldn't reach {}: {}", self.base, error_chain(&e)))
        })?;
        let status = res.status();
        if status.is_success() {
            if status == StatusCode::NO_CONTENT {
                return serde_json::from_str("null")
                    .map_err(|e| AppError::Remote(format!("unexpected empty answer: {e}")));
            }
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
            _ => AppError::Remote(message),
        })
    }
}

/// The innermost cause too: reqwest's own message for a refused handshake
/// is just "error sending request".
fn error_chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        out.push_str(": ");
        out.push_str(&s.to_string());
        source = s.source();
    }
    out
}

/// Connects to a machine being added without knowing its key yet, and
/// returns the key's fingerprint (for the person to compare with the one the
/// agent printed) and what the agent says about itself.
pub async fn probe(base: &str, identity: &Identity) -> Result<(Fingerprint, Hello), AppError> {
    let server = PinnedServer::learning(&crate::link::provider());
    let agent = Agent::new(base, identity, server.clone())?;
    let hello: Hello = agent
        .call(Method::GET, agent.url(&["hello"]), None::<&()>, "hello")
        .await?;
    let fingerprint = server
        .seen()
        .ok_or_else(|| AppError::Remote("the machine showed no certificate".into()))?;
    Ok((fingerprint, hello))
}

/// Pairs with the agent at `base`, whose key must be `pin`, using the code
/// the agent printed. On success the agent has pinned this hub's key.
pub async fn pair(
    base: &str,
    identity: &Identity,
    pin: Fingerprint,
    code: &str,
    hub_name: &str,
) -> Result<Paired, AppError> {
    let server = PinnedServer::pinned(pin, &crate::link::provider());
    let agent = Agent::new(base, identity, server)?;
    let body = PairRequest {
        code: code.to_string(),
        hub_name: hub_name.to_string(),
    };
    agent
        .call(Method::POST, agent.url(&["pair"]), Some(&body), "pair")
        .await
}

impl RemoteMachine {
    /// A linked machine: the agent at `base`, whose key must be `pin`.
    pub fn new(base: &str, identity: &Identity, pin: Fingerprint) -> Result<Self, AppError> {
        let server = PinnedServer::pinned(pin, &crate::link::provider());
        Ok(Self {
            agent: Agent::new(base, identity, server)?,
        })
    }

    async fn get<T: DeserializeOwned>(&self, url: Url, subject: &str) -> Result<T, AppError> {
        self.agent
            .call(Method::GET, url, None::<&()>, subject)
            .await
    }

    pub async fn facts(&self) -> Result<Facts, AppError> {
        self.get(self.agent.url(&["info"]), "info").await
    }

    pub async fn records(&self, kinds: &[UnitKind]) -> Result<Vec<UnitRecord>, AppError> {
        let mut url = self.agent.url(&["units"]);
        let kinds: Vec<&str> = kinds.iter().map(|k| k.extension()).collect();
        url.query_pairs_mut().append_pair("kinds", &kinds.join(","));
        self.get(url, "units").await
    }

    pub async fn status(&self, file_name: &str) -> Result<UnitStatus, AppError> {
        let url = self.agent.url(&["units", file_name, "status"]);
        self.get(url, file_name).await
    }

    pub async fn act(&self, file_name: &str, action: Action) -> Result<UnitStatus, AppError> {
        let url = self.agent.url(&["units", file_name, action.as_str()]);
        self.agent
            .call(Method::POST, url, None::<&()>, file_name)
            .await
    }

    pub async fn logs(&self, file_name: &str, lines: u32) -> Result<Vec<LogLine>, AppError> {
        let mut url = self.agent.url(&["units", file_name, "logs"]);
        url.query_pairs_mut()
            .append_pair("lines", &lines.to_string());
        self.get(url, file_name).await
    }

    /// Asks the agent to forget this hub.
    pub async fn unpair(&self) -> Result<(), AppError> {
        self.agent
            .call(
                Method::DELETE,
                self.agent.url(&["pair"]),
                None::<&()>,
                "pair",
            )
            .await
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::link::listener::TlsListener;
    use crate::link::trust::{HubStore, Permission};
    use crate::machine::Machine;
    use crate::machine::api::{ApiState, router};
    use crate::machine::fake::FakeMachine;

    /// An agent serving the real API over mutual TLS on a loopback port.
    pub struct TestAgent {
        pub url: String,
        pub identity: Identity,
        pub hubs: Arc<HubStore>,
        _dir: tempfile::TempDir,
    }

    pub async fn spawn_agent(fake: FakeMachine) -> TestAgent {
        let dir = tempfile::tempdir().unwrap();
        let identity = Identity::generate().unwrap();
        let hubs = Arc::new(HubStore::load(dir.path()).unwrap());
        let app = router(ApiState {
            machine: Machine::Fake(fake),
            hubs: hubs.clone(),
        });
        let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = tcp.local_addr().unwrap();
        let listener = TlsListener::new(tcp, identity.server_config().unwrap()).unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<crate::link::listener::Peer>(),
            )
            .await
            .unwrap()
        });
        TestAgent {
            url: format!("https://{addr}"),
            identity,
            hubs,
            _dir: dir,
        }
    }

    pub fn fake() -> FakeMachine {
        FakeMachine::with_units(&[
            ("", "web.container", "[Container]\nImage=nginx\n"),
            ("media", "data.volume", "[Volume]\n"),
        ])
    }

    /// Pairs a fresh hub identity with `agent` and returns it linked.
    async fn linked(agent: &TestAgent, permission: Permission) -> (Identity, Machine) {
        let hub = Identity::generate().unwrap();
        let code = agent.hubs.issue_code(permission).unwrap();
        let (fp, hello) = probe(&agent.url, &hub).await.unwrap();
        assert_eq!(fp, agent.identity.fingerprint);
        assert_eq!(hello.hostname, "fake01");
        let paired = pair(&agent.url, &hub, fp, &code, "hub01").await.unwrap();
        assert_eq!(paired.permission, permission);
        let remote = RemoteMachine::new(&agent.url, &hub, fp).unwrap();
        (hub, Machine::Remote(remote))
    }

    #[tokio::test]
    async fn a_paired_hub_lists_parses_and_drives_the_agents_units() {
        let fake = fake();
        let agent = spawn_agent(fake.clone()).await;
        let (_hub, remote) = linked(&agent, Permission::Full).await;

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

        assert!(matches!(
            remote.status("nope.container").await,
            Err(AppError::NotFound(f)) if f == "nope.container"
        ));
    }

    #[tokio::test]
    async fn a_read_only_hub_can_look_but_not_act() {
        let fake = fake();
        let agent = spawn_agent(fake.clone()).await;
        let (_hub, remote) = linked(&agent, Permission::ReadOnly).await;
        assert!(remote.status("web.container").await.is_ok());
        assert!(matches!(
            remote.act("web.container", Action::Start).await,
            Err(AppError::Remote(m)) if m.contains("read-only")
        ));
        assert!(fake.actions().is_empty());
    }

    #[tokio::test]
    async fn strangers_wrong_codes_and_changed_keys_are_refused() {
        let agent = spawn_agent(fake()).await;

        // A hub that never paired can say hello but nothing else.
        let stranger = Identity::generate().unwrap();
        let unpaired =
            RemoteMachine::new(&agent.url, &stranger, agent.identity.fingerprint).unwrap();
        assert!(matches!(
            unpaired.facts().await,
            Err(AppError::Remote(m)) if m.contains("isn't paired")
        ));

        // A wrong code pairs nothing.
        agent.hubs.issue_code(Permission::Full).unwrap();
        let fp = agent.identity.fingerprint;
        assert!(matches!(
            pair(&agent.url, &stranger, fp, "AAAA-BBBB-CCCC", "x").await,
            Err(AppError::Remote(m)) if m.contains("wrong")
        ));

        // Pinned to some other key, the hub won't talk to this agent at all.
        let other = Identity::generate().unwrap().fingerprint;
        let (_hub, _) = linked(&agent, Permission::Full).await;
        let impostor = RemoteMachine::new(&agent.url, &stranger, other).unwrap();
        assert!(matches!(
            impostor.facts().await,
            Err(AppError::Remote(m)) if m.contains("key changed")
        ));

        // Plain http is refused before any connection.
        assert!(RemoteMachine::new("http://127.0.0.1:1", &stranger, fp).is_err());
    }

    #[tokio::test]
    async fn unpairing_makes_the_agent_forget_the_hub() {
        let agent = spawn_agent(fake()).await;
        let (_hub, remote) = linked(&agent, Permission::Full).await;
        let Machine::Remote(r) = &remote else {
            unreachable!()
        };
        r.unpair().await.unwrap();
        assert!(agent.hubs.hubs().is_empty());
        assert!(remote.facts().await.is_err());
    }

    #[test]
    fn urls_keep_the_base_path_and_encode_each_segment() {
        let id = Identity::generate().unwrap();
        let m = RemoteMachine::new("https://h:1/agent/", &id, id.fingerprint).unwrap();
        assert_eq!(
            m.agent.url(&["units", "a b.container", "status"]).as_str(),
            "https://h:1/agent/api/v1/units/a%20b.container/status"
        );
        assert!(RemoteMachine::new("not a url", &id, id.fingerprint).is_err());
    }
}
