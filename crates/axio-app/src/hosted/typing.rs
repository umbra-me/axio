//! Typing at a hosted agent on a person's behalf.
//!
//! Only one thing does this — a group handing its prompt to each member — and
//! it is here rather than in `mod.rs` because the timing is the whole of it.

use super::Hosted;
use crate::model::AppError;

impl Hosted {
    /// Type `text` into a terminal and submit it, once the agent is ready.
    ///
    /// Written straight after spawn, the text lands in the agent's composer
    /// and the Enter is lost: the interface was still starting when it
    /// arrived. So this waits — for output to appear and then go quiet for a
    /// moment, which is what a painted prompt looks like from outside — types
    /// the text, pauses, and sends the carriage return as its own write. The
    /// pause matters twice: several agents treat text and Enter in one burst
    /// as a paste, and none of them submit a paste. Fifteen seconds with no
    /// output and the text is sent anyway; a dead terminal cannot get worse.
    pub fn submit_when_ready(&self, id: &str, text: String) -> Result<(), AppError> {
        let session = self.get(id)?;
        tokio::spawn(async move {
            let started = std::time::Instant::now();
            let mut seen = 0u64;
            let mut quiet_since: Option<std::time::Instant> = None;
            loop {
                let (_, cursor) = session.read_from(seen);
                if cursor != seen {
                    seen = cursor;
                    quiet_since = Some(std::time::Instant::now());
                } else if let Some(q) = quiet_since
                    && q.elapsed() >= std::time::Duration::from_millis(600)
                {
                    break;
                }
                if started.elapsed() >= std::time::Duration::from_secs(15) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            let _ = session.write(text.as_bytes());
            tokio::time::sleep(std::time::Duration::from_millis(350)).await;
            let _ = session.write(b"\r");
        });
        Ok(())
    }
}
