//! LLM provider management and routing.

pub mod anthropic;
<<<<<<< ours
pub mod history_repair;
=======
pub mod gemini;
>>>>>>> theirs
pub mod manager;
pub mod model;
pub mod pricing;
pub mod providers;
pub mod record;
pub mod routing;
pub mod usage;

pub use manager::LlmManager;
pub use model::SpacebotModel;
pub use record::{DebugContext, PromptRecord, PromptRecordStore};
pub use routing::RoutingConfig;
