# Internal: Feature Pinning Table

Mirror of the [`FEATURES`] registry in
`crates/codegen/xai-grok-config-types/src/registry.rs`. The
`registered_features_are_documented` test fails when a registered feature key
is missing from this table. Update both together.

| Feature key | Config path | Default |
|-------------|-------------|---------|
| `session_search` | `features.session_search` | enabled |
| `lsp_tools` | `features.lsp_tools` | disabled |
| `web_fetch` | `features.web_fetch` | disabled |
| `session_recap` | `features.session_recap` | enabled |
| `ask_user_question` | `features.ask_user_question` | enabled |
| `voice_mode` | `features.voice_mode` | enabled |
| `write_file` | `features.write_file` | enabled |
| `feedback` | `features.feedback` | enabled |
| `turn_summary` | `features.turn_summary` | enabled |
| `cancel_rewind` | `features.cancel_rewind` | enabled |
| `compaction_verbatim_input` | `features.compaction_verbatim_input` | enabled |
| `two_pass_compaction` | `features.two_pass_compaction` | disabled |
| `auto_wake` | `features.auto_wake` | enabled |
| `subagent_worktree_snapshot` | `features.subagent_worktree_snapshot` | disabled |

Enterprise deployments can pin any of these per managed-config layer; remote
settings targeting rules override the local defaults listed above.
