use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio_events::prelude::*;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Event, Serialize, Deserialize)]
struct UserDataRequest {
    user_id: u64,
}

#[derive(Clone, Debug, Event, PartialEq, Serialize, Deserialize)]
struct UserDataResponse {
    name: String,
}

#[tokio::test]
async fn test_rpc_request_reply() -> Result<()> {
    let bus = EventBusBuilder::new().build().await?;

    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_clone = call_count.clone();

    // Register the responder
    let _responder = bus
        .respond(move |req: UserDataRequest| {
            let count = call_count_clone.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);

                UserDataResponse {
                    name: format!("User {}", req.user_id),
                }
            }
        })
        .await?;

    // Make a request and await the response
    let response: UserDataResponse = bus.request(UserDataRequest { user_id: 42 }).await?;

    assert_eq!(response.name, "User 42");
    assert_eq!(call_count.load(Ordering::SeqCst), 1);

    // Make another request
    let response2: UserDataResponse = bus.request(UserDataRequest { user_id: 100 }).await?;

    assert_eq!(response2.name, "User 100");
    assert_eq!(call_count.load(Ordering::SeqCst), 2);

    Ok(())
}
