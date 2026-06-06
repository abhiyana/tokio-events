use serde::{Deserialize, Serialize};
use tokio_events::prelude::*;

// -----------------------------------------------------------------------------
// EVENT 1: RPC Request
// -----------------------------------------------------------------------------
#[derive(Debug, Clone, Serialize, Deserialize, Event, Remote)]
#[remote(topic = "v2.users.profile.request")]
pub struct GetProfileRequest {
    pub user_id: u64,
}

// -----------------------------------------------------------------------------
// EVENT 2: RPC Response
// -----------------------------------------------------------------------------
#[derive(Debug, Clone, Serialize, Deserialize, Event, Remote)]
#[remote(topic = "v2.users.profile.response")]
pub struct GetProfileResponse {
    pub name: String,
    pub is_vip: bool,
    pub error_msg: Option<String>,
}

// -----------------------------------------------------------------------------
// EVENT 3: Topic-Routed Event
// -----------------------------------------------------------------------------
#[derive(Debug, Clone, Serialize, Deserialize, Event, Remote)]
#[remote(topic = "v2.orders.*.created")]
pub struct OrderCreated {
    pub order_id: String,
    pub user_id: u64,
    pub amount: f64,
}

// -----------------------------------------------------------------------------
// EVENT 4: Local Scheduled Event
// -----------------------------------------------------------------------------
#[derive(Debug, Clone, Serialize, Deserialize, Event)]
pub struct SendSurveyEmail {
    pub order_id: String,
    pub user_id: u64,
}
