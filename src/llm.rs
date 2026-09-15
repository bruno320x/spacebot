//! LLM provider management and routing.

pub mod anthropic;
pub mod history_repair;
pub mod manager;
pub mod model;
pub mod pricing;
pub mod providers;
pub mod record;
pub mod routing;
<<<<<<< ours
pub mod usage;
=======
pub mod transcription;
>>>>>>> theirs

pub use manager::LlmManager;
pub use model::SpacebotModel;
pub use record::{DebugContext, PromptRecord, PromptRecordStore};
pub use routing::RoutingConfig;
// Re-export types from transcription module
pub use transcription::{TranscriptionRequest, TranscriptionResponse, transcribe_audio};
