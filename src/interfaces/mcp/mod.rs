//! MCP protocol adaptation over an owned stdio session.
mod protocol;
mod session;

pub use protocol::handle;
pub use session::serve;
