//! Connection-owned discovery. Observation never authorizes a forwarding action.
use super::{SourceToken, SourceWorkspace};
use splitty_transport::budget::ConnectionBudget;
use std::{io, sync::Arc};

#[derive(Clone, Debug)]
pub struct ObservedPort {
    port: u16,
    session: String,
    process: String,
}
impl ObservedPort {
    pub fn port(&self) -> u16 {
        self.port
    }
    pub fn session_id(&self) -> &str {
        &self.session
    }
    pub fn process(&self) -> &str {
        &self.process
    }
}
#[derive(Debug)]
pub struct PortObservation {
    source: SourceToken,
    ports: Vec<ObservedPort>,
}
impl PortObservation {
    pub fn source(&self) -> SourceToken {
        self.source
    }
    pub fn ports(&self) -> &[ObservedPort] {
        &self.ports
    }
}
impl SourceWorkspace {
    pub fn port_observation(&self) -> Option<&PortObservation> {
        self.ports.as_deref().map(|value| &**value)
    }
    pub(crate) fn clear_port_observation(&mut self) {
        self.ports = None;
    }
    pub(crate) fn observe_ports(
        &mut self,
        source: SourceToken,
        json: &serde_json::Value,
        budget: &ConnectionBudget,
    ) -> io::Result<bool> {
        if source != self.source_token() || !self.is_ready() {
            return Ok(false);
        }
        let invalid = || {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid listening-port observation",
            )
        };
        let rows = json
            .get("ports")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(invalid)?;
        if rows.len() > 256 {
            return Err(invalid());
        }
        let mut seen = [0_u64; 1024];
        let mut bytes = std::mem::size_of::<PortObservation>()
            + rows.len() * std::mem::size_of::<ObservedPort>();
        for row in rows {
            let port = row["port"]
                .as_u64()
                .filter(|port| (1..=65535).contains(port))
                .ok_or_else(invalid)?;
            let session = row["sessionId"].as_str().ok_or_else(invalid)?;
            let process = row["process"].as_str().ok_or_else(invalid)?;
            if session.is_empty()
                || session.len() > 256
                || process.len() > 256
                || session.chars().any(char::is_control)
                || process.chars().any(char::is_control)
            {
                return Err(invalid());
            }
            let port = usize::try_from(port).map_err(|_| invalid())?;
            if seen[port / 64] & (1 << (port % 64)) != 0 {
                return Err(invalid());
            }
            seen[port / 64] |= 1 << (port % 64);
            bytes += session.len() + process.len();
        }
        let lease = budget.reserve(bytes, false).map_err(|_| {
            io::Error::new(
                io::ErrorKind::OutOfMemory,
                "Listening-port observation exceeds capacity",
            )
        })?;
        let ports = rows
            .iter()
            .map(|row| ObservedPort {
                port: u16::try_from(row["port"].as_u64().expect("validated port"))
                    .expect("validated port range"),
                session: row["sessionId"].as_str().expect("validated session").into(),
                process: row["process"].as_str().expect("validated process").into(),
            })
            .collect();
        self.ports = Some(Arc::new(lease.charge(PortObservation { source, ports })));
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;
    #[test]
    fn source_observations_are_atomic_charged_and_connection_scoped() {
        let budget = ConnectionBudget::default();
        let mut source = SourceWorkspace::new(Uuid::new_v4());
        let owner = source.source_token();
        assert!(source.port_observation().is_none());
        let valid = json!({"ports":[{"port":3000,"sessionId":"s","process":"node"}]});
        assert!(source.observe_ports(owner, &valid, &budget).unwrap());
        let charged = budget.used();
        assert!(charged > 0);
        for invalid in [
            json!({}),
            json!({"ports":[{"port":0,"sessionId":"s","process":"node"}]}),
            json!({"ports":[{"port":3,"sessionId":"s","process":"x"},{"port":3,"sessionId":"s","process":"x"}]}),
            json!({"ports":[{"port":4,"sessionId":"s","process":"x".repeat(257)}]}),
            json!({"ports":(1..258).map(|port|json!({"port":port,"sessionId":"s","process":"x"})).collect::<Vec<_>>()}),
        ] {
            assert!(source.observe_ports(owner, &invalid, &budget).is_err());
            assert_eq!(source.port_observation().unwrap().ports()[0].port(), 3000);
            assert_eq!(budget.used(), charged);
        }
        let old = source.ports.clone();
        source.reset_connection(Uuid::new_v4());
        assert!(source.port_observation().is_none());
        assert!(!source.observe_ports(owner, &valid, &budget).unwrap());
        drop(old);
        assert_eq!(budget.used(), 0);
        assert!(source
            .observe_ports(source.source_token(), &json!({"ports":[]}), &budget)
            .unwrap());
        assert!(source.port_observation().unwrap().ports().is_empty());
        source.clear_port_observation();
        assert!(source.port_observation().is_none());
        assert_eq!(budget.used(), 0);
    }
    #[test]
    fn pressure_rejects_replacement_before_copying() {
        // Saturate a private process budget, not the one shared by the other
        // tests of this binary.
        let budget = ConnectionBudget::new(
            splitty_transport::budget::ProcessBudget::new(
                splitty_transport::wire::CONNECTION_BYTES,
                splitty_transport::wire::CONNECTION_CONTROL_BYTES,
            ),
            splitty_transport::wire::CONNECTION_BYTES,
            splitty_transport::wire::CONNECTION_CONTROL_BYTES,
        );
        let mut source = SourceWorkspace::new(Uuid::new_v4());
        let held = budget
            .reserve(
                splitty_transport::wire::CONNECTION_BYTES
                    - splitty_transport::wire::CONNECTION_CONTROL_BYTES,
                false,
            )
            .unwrap();
        assert!(source
            .observe_ports(
                source.source_token(),
                &serde_json::json!({"ports":[]}),
                &budget
            )
            .is_err());
        assert!(source.port_observation().is_none());
        drop(held);
    }
}
