use crate::db::Db;
use crate::transport::CircuitBreakers;
use std::sync::Arc;
use tokio::time::{interval, Duration};
use tracing::{debug, info, warn};

pub fn start_heartbeat(db: Db, circuits: Arc<CircuitBreakers>) {
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(30));
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .expect("heartbeat client");

        let mut last_caps_check: std::collections::HashMap<String, i64> =
            std::collections::HashMap::new();

        loop {
            ticker.tick().await;

            let machines = match db.list() {
                Ok(m) => m,
                Err(e) => {
                    warn!("heartbeat: failed to list machines: {}", e);
                    continue;
                }
            };

            for machine in machines {
                let now = chrono::Utc::now().timestamp();

                // Only ping agent-capable machines
                let url = match &machine.agent_url {
                    Some(u) => format!("{}/health", u.trim_end_matches('/')),
                    None => continue,
                };

                match client.get(&url).send().await {
                    Ok(resp) if resp.status().is_success() => {
                        info!("heartbeat: {} online", machine.label);
                        let _ = db.update_heartbeat(&machine.id, "online", now);
                        circuits.record_success(&machine.id);

                        // Re-fetch capabilities if > 1h ago
                        let last = last_caps_check.get(&machine.id).copied().unwrap_or(0);
                        if now - last > 3600 {
                            if let Some(agent_url) = &machine.agent_url {
                                let caps_url = format!("{}/capabilities", agent_url.trim_end_matches('/'));
                                if let Ok(cr) = client.get(&caps_url).send().await {
                                    if let Ok(caps) = cr.json::<crate::db::Capabilities>().await {
                                        let _ = db.update_capabilities(&machine.id, &caps);
                                        last_caps_check.insert(machine.id.clone(), now);
                                    }
                                }
                            }
                        }
                    }
                    Ok(resp) => {
                        warn!("heartbeat: {} returned {}", machine.label, resp.status());
                        let _ = db.update_heartbeat(&machine.id, "unreachable", now);
                        circuits.record_failure(&machine.id);
                    }
                    Err(e) => {
                        debug!("heartbeat: {} unreachable: {}", machine.label, e);
                        let _ = db.update_heartbeat(&machine.id, "unreachable", now);
                        circuits.record_failure(&machine.id);
                    }
                }
            }
        }
    });
}
