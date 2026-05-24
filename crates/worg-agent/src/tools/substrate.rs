//! `substrate_push` — commit + push the current workdir to the
//! Workbooks substrate (git-backed). Implemented as a shell-out to
//! the `workbook git push` CLI subcommand, which handles capability
//! minting + auth against the substrate broker.

use serde_json::json;

use crate::tools::shell::ShellTool;

/// `workbook git push` wrapper. Commits any unstaged changes in the
/// workdir + pushes to the active substrate org. Auth is handled by
/// the workbook CLI's credential helper (wb-kven).
pub fn push() -> ShellTool {
    ShellTool::new(
        "substrate_push",
        "Stage every change in the workdir, commit with the provided \
         message, and push to the substrate's main branch. Auth is \
         handled by the workbook CLI's credential helper — the agent \
         needs no explicit token. Returns the workbook CLI's stdout.",
        json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "Commit message body."
                },
                "org": {
                    "type": "string",
                    "description": "Optional substrate org slug to target. Defaults to the active org."
                }
            },
            "required": ["message"]
        }),
        "workbook",
    )
    .with_argv_prefix(["git", "push"])
    .with_flag("message", "--message")
    .with_flag("org", "--org")
}

pub fn register_all(registry: &mut crate::tool_registry::ToolRegistry) {
    registry.register(push());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::Tool;

    #[test]
    fn push_wrapper_has_right_name() {
        assert_eq!(push().name(), "substrate_push");
    }
}
