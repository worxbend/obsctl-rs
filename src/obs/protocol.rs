use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const OPCODE_HELLO: u8 = 0;
pub const OPCODE_IDENTIFY: u8 = 1;
pub const OPCODE_IDENTIFIED: u8 = 2;
pub const OPCODE_REIDENTIFY: u8 = 3;
pub const OPCODE_EVENT: u8 = 5;
pub const OPCODE_REQUEST: u8 = 6;
pub const OPCODE_REQUEST_RESPONSE: u8 = 7;
pub const OPCODE_REQUEST_BATCH: u8 = 8;
pub const OPCODE_REQUEST_BATCH_RESPONSE: u8 = 9;

#[derive(Debug, Serialize, Deserialize)]
pub struct ObsMessage {
    pub op: u8,
    pub d: Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HelloData {
    #[serde(rename = "obsWebSocketVersion")]
    pub obs_web_socket_version: String,
    #[serde(rename = "rpcVersion")]
    pub rpc_version: u32,
    pub authentication: Option<HelloAuthentication>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HelloAuthentication {
    pub challenge: String,
    pub salt: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IdentifyData {
    #[serde(rename = "rpcVersion")]
    pub rpc_version: u32,
    pub authentication: Option<String>,
    #[serde(rename = "eventSubscriptions", skip_serializing_if = "Option::is_none")]
    pub event_subscriptions: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IdentifiedData {
    #[serde(rename = "negotiatedRpcVersion")]
    pub negotiated_rpc_version: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RequestData {
    #[serde(rename = "requestType")]
    pub request_type: String,
    #[serde(rename = "requestId")]
    pub request_id: String,
    #[serde(rename = "requestData", skip_serializing_if = "Option::is_none")]
    pub request_data: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RequestResponseData {
    #[serde(rename = "requestType")]
    pub request_type: String,
    #[serde(rename = "requestId")]
    pub request_id: String,
    #[serde(rename = "requestStatus")]
    pub request_status: RequestStatus,
    #[serde(rename = "responseData")]
    pub response_data: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RequestStatus {
    pub result: bool,
    pub code: u32,
    pub comment: Option<String>,
}
