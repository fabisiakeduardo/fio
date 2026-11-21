use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RouteSource {
    AppName(String),
    CaptureDevice(String),
    VirtualMonitor(Uuid),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RouteTarget {
    VirtualDevice(Uuid),
    PhysicalSink(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Route {
    pub id: Uuid,
    pub source: RouteSource,
    pub target: RouteTarget,
}
