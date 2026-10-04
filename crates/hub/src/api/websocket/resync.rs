use blazar_core_types::api::ServerEvent;

pub fn lag_notification() -> ServerEvent {
    ServerEvent::ResyncRequired
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn websocket_lag_requires_full_client_state_recovery() {
        assert_eq!(format!("{:?}", lag_notification()), "ResyncRequired");
    }
}
