use serde::{Deserialize, Serialize};
use tokio_events::Event;

#[derive(Event, Serialize, Deserialize, Debug, Clone)]
struct SimpleEvent {
    pub message: String,
}

#[derive(Event, Serialize, Deserialize, Debug, Clone)]
#[event(event_type = "custom.namespace.MyCustomEvent")]
struct CustomEvent {
    pub id: u32,
}

#[test]
fn test_macro_default_event_type() {
    assert_eq!(
        SimpleEvent::event_type(),
        concat!(module_path!(), "::SimpleEvent")
    );
}

#[test]
fn test_macro_custom_event_type() {
    assert_eq!(CustomEvent::event_type(), "custom.namespace.MyCustomEvent");
}
