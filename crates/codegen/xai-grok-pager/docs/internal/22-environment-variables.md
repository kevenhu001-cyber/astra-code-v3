# Internal: Feature Environment Variables

Mirror of the `env` column of the [`FEATURES`] registry in
`crates/codegen/xai-grok-config-types/src/registry.rs`. The
`registered_features_are_documented` test fails when a registered env var is
missing from this table. Update both together.

| Environment variable | Feature key | Default |
|----------------------|-------------|---------|
| `GROK_SESSION_SEARCH` | `session_search` | enabled |
| `GROK_LSP_TOOLS` | `lsp_tools` | disabled |
| `GROK_WEB_FETCH` | `web_fetch` | disabled |
| `GROK_SESSION_RECAP` | `session_recap` | enabled |
| `GROK_ASK_USER_QUESTION` | `ask_user_question` | enabled |
| `GROK_VOICE_MODE` | `voice_mode` | enabled |
| `GROK_WRITE_FILE` | `write_file` | enabled |
| `GROK_FEEDBACK_ENABLED` | `feedback` | enabled |
| `GROK_TURN_SUMMARY` | `turn_summary` | enabled |
| `GROK_CANCEL_REWIND` | `cancel_rewind` | enabled |
| `GROK_COMPACTION_VERBATIM_INPUT` | `compaction_verbatim_input` | enabled |
| `GROK_TWO_PASS_COMPACTION` | `two_pass_compaction` | disabled |
| `GROK_AUTO_WAKE` | `auto_wake` | enabled |
| `GROK_SUBAGENT_WORKTREE_SNAPSHOT` | `subagent_worktree_snapshot` | disabled |

Set a variable to `1`/`0` to override the default for the session; config
files take precedence over unset variables.
