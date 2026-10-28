//! C# fixes use the daemon/offline service and the same source.replace op history.
use crate::{
    commands::protocol::{self, Call},
    params,
};
use serde_json::json;

#[derive(clap::Args)]
pub struct FixArgs {
    /// C# reference or project storage paths (default: every inventoried source).
    paths: Vec<String>,
    /// Analyzer codes to fix; repeat or separate by commas (default: all shipped fixes).
    #[arg(long = "code", value_delimiter = ',')]
    codes: Vec<String>,
    /// Preview replacements and ops without changing source files.
    #[arg(long)]
    dry_run: bool,
    /// Authoring document anchoring the transaction (default: main scene).
    #[arg(long)]
    doc: Option<String>,
    /// Reason recorded in the attributed transaction.
    #[arg(long)]
    why: Option<String>,
}
impl FixArgs {
    pub fn call(self) -> Call {
        Call {
            method: "source.fix",
            params: params(
                json!({"paths":(!self.paths.is_empty()).then_some(self.paths),"codes":(!self.codes.is_empty()).then_some(self.codes),"apply":!self.dry_run,"doc":self.doc,"why":self.why.unwrap_or_else(|| "Apply shipped C# analyzer code fixes".into())}),
            ),
            summary: protocol::pretty,
        }
    }
}
