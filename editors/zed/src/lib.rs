//! The tsox extension for Zed: tells Zed to run `tsox-lsp` from the project's `PATH`.

use zed_extension_api::{self as zed, LanguageServerId, Result};

struct Tsox;

impl zed::Extension for Tsox {
    fn new() -> Self {
        Tsox
    }

    fn language_server_command(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let command = worktree.which("tsox-lsp").ok_or_else(|| {
            "tsox-lsp was not found on your PATH. Install it from \
             https://github.com/tsoxide/tsox and restart Zed."
                .to_string()
        })?;
        Ok(zed::Command {
            command,
            args: vec!["--stdio".to_string()],
            env: worktree.shell_env(),
        })
    }
}

zed::register_extension!(Tsox);
