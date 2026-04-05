pub mod parse;
pub mod policy;
pub mod render;

use serde::Serialize;

use tt_core::{ReportParseResult, TTResult};

pub const ASSIGNMENT_COMMUNICATION_PACKET_SCHEMA_VERSION: &str =
    "assignment_communication_packet.v1";
pub const ASSIGNMENT_PROMPT_TEMPLATE_VERSION: &str = "assignment_prompt.v1";
pub const WORKER_REPORT_CONTRACT_SCHEMA_VERSION: &str = "worker_report_contract.v1";
pub const WORKER_REPORT_ENVELOPE_SCHEMA_VERSION: &str = "worker_report_envelope.v1";
pub const REPORT_MARKER_BEGIN: &str = "TT_REPORT_BEGIN";
pub const REPORT_MARKER_END: &str = "TT_REPORT_END";

#[derive(Debug, Clone)]
pub struct EnvelopeExtraction {
    pub json_payload: Option<String>,
    pub surrounding_text: bool,
}

#[derive(Debug, Clone)]
pub struct EnvelopeValidationResult {
    pub parse_result: ReportParseResult,
    pub structural_issues: Vec<String>,
    pub semantic_issues: Vec<String>,
    pub policy_violations: Vec<String>,
    pub needs_supervisor_review: bool,
}

fn stable_fingerprint_bytes(bytes: &[u8]) -> String {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{hash:016x}")
}

pub fn stable_fingerprint(input: &str) -> String {
    stable_fingerprint_bytes(input.as_bytes())
}

pub fn json_fingerprint<T: Serialize>(value: &T) -> TTResult<String> {
    Ok(stable_fingerprint(&serde_json::to_string(value)?))
}
