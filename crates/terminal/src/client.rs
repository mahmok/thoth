use core::ops::{Deref, DerefMut};

use thoth_core::harness::Harness;
use thoth_core::session::Session;
use thoth_core::session::view::View;

/// A Harness and the one session this terminal is showing. Which session is viewed is the
/// terminal's business; the Harness only knows its sessions by id.
pub struct Client {
    harness: Harness,
    session: u64,
}

impl Client {
    /// Abandons a login in progress and interrupts the viewed session's turn.
    pub fn cancel(&mut self) {
        self.harness.cancel_login();
        self.harness.cancel_session(self.session);
    }

    /// Closes `session`, moving the view to the lowest-numbered one left if it was shown. The
    /// last session is never closed.
    pub fn close(&mut self, session: u64) -> bool {
        let ids: Vec<u64> = self.harness.sessions().iter().map(|&(id, _)| id).collect();
        if ids.len() < 2 || !ids.contains(&session) {
            return false;
        }
        let _closed = self.harness.close_session(session);
        if self.session == session {
            self.session = ids.into_iter().find(|id| *id != session).unwrap_or(self.session);
        }
        true
    }

    /// Views the harness's first session, or starts one on its first agent.
    pub fn open(mut harness: Harness) -> Result<Self, &'static str> {
        let session = if let Some(&(id, _)) = harness.sessions().first() {
            id
        } else {
            let persona = harness.list_personas().first().map(|p| p.key.clone());
            persona
                .and_then(|name| harness.create_session(&name))
                .ok_or("there is no persona to start a session with")?
        };
        Ok(Self { harness, session })
    }

    #[expect(clippy::expect_used, reason = "`close` keeps the viewed session in the harness")]
    pub fn session(&self) -> &Session {
        self.harness.session(self.session).expect("the viewed session exists")
    }

    pub const fn session_id(&self) -> u64 {
        self.session
    }

    #[expect(clippy::expect_used, reason = "`close` keeps the viewed session in the harness")]
    pub fn session_mut(&mut self) -> &mut Session {
        self.harness.session_mut(self.session).expect("the viewed session exists")
    }

    pub fn submit(&mut self, shown: String, full: String) -> bool {
        self.harness.submit(self.session, shown, full)
    }

    /// Shows `session` instead; false when there is no such session.
    pub fn switch(&mut self, session: u64) -> bool {
        let exists = self.harness.session(session).is_some();
        if exists {
            self.session = session;
        }
        exists
    }

    #[expect(clippy::expect_used, reason = "`close` keeps the viewed session in the harness")]
    pub fn view(&self) -> &View {
        self.harness.session(self.session).expect("the viewed session exists").view()
    }
}

impl Deref for Client {
    type Target = Harness;

    fn deref(&self) -> &Harness {
        &self.harness
    }
}

impl DerefMut for Client {
    fn deref_mut(&mut self) -> &mut Harness {
        &mut self.harness
    }
}
