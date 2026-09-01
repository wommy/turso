# MCP guard audit: which direction is tested

> **A snapshot, not a live record.** Taken against `claude/mcp-http-transport`
> at `da516ea` and `claude/mcp-v2-protocol` at `e3c31ec`. Both branches have
> moved since. The gaps it found are tracked at
> [#40](https://github.com/wommy/turso/issues/40), which is the record that
> stays current; this table is the evidence behind it and the list of what was
> checked and found clean, which is what stops the next sweep redoing the same
> ground. Re-run the sweep rather than trusting these rows — the method is in
> [`../workflows/periodic-sweeps.md`](../workflows/periodic-sweeps.md).

Every place the MCP protocol layer or HTTP transport refuses input, checked
against two questions: is there a test proving it refuses bad input, and a
separate test proving it still accepts good input. `none` means neither
adjacent tests nor a repo-wide grep for the guard's message or error code
turned one up. Background for why both directions matter, with the two
incidents that motivated this table: [`background-agents.md`, "Negative proof
has to isolate the test"](background-agents.md#negative-proof-has-to-isolate-the-test).
The decision this table backs is [ADR 0005](../adr/0005-both-directions-of-a-guard-need-a-test.md).

Snapshot taken 2026-09-01. `turso-http-mcp` was being committed to by another
agent while this was read; treat the HTTP-transport half as a point-in-time
read, not a live one.

## Protocol layer — `turso-mcp-v2/cli/mcp/`

| Guard | Where | Refuses bad input | Accepts good input |
|---|---|---|---|
| Malformed JSON body → `PARSE_ERROR` | `mod.rs` `handle_message` | none | every other test (all send valid JSON) |
| `id: null` → `INVALID_REQUEST` | `mod.rs` `handle_message` | `a_null_id_is_not_a_notification` | `a_notification_is_answered_with_nothing` (absent id) + every test with a real id |
| `_meta` wrong shape (not an object / `protocolVersion` not a string / `clientCapabilities` not an object) | `protocol.rs` `check_meta_shape` | `a_meta_field_of_the_wrong_type_is_refused_rather_than_read_as_absent` (all 3 shapes) | `a_v2_client_calls_a_tool_with_no_handshake_at_all` and most other tests (well-formed `_meta`) |
| Unsupported protocol version → `UNSUPPORTED_PROTOCOL_VERSION` | `protocol.rs` `check_protocol_version` | `a_version_we_do_not_speak_is_refused_with_the_ones_we_do` | `a_v2_client_calls_a_tool_with_no_handshake_at_all` (declares `PROTOCOL_V2`, accepted) |
| v2 client missing `clientCapabilities` → `INVALID_PARAMS` | `protocol.rs` `check_client_capabilities` | `client_capabilities_are_required_of_v2_clients_only` (strict case) | same test's lenient case, and `a_v2_client_calls_a_tool_with_no_handshake_at_all` (v2 **with** capabilities) |
| Unknown JSON-RPC method → `METHOD_NOT_FOUND` | `mod.rs` `handle_request` | `an_unknown_method_is_method_not_found` | every test naming `tools/list`, `tools/call`, `initialize`, `ping`, `server/discover` |
| `tools/call` with no `params` → `INVALID_PARAMS` | `tools.rs` `handle_call_tool` | none | every `tools/call` test (all send `params`) |
| `tools/call` params that don't deserialize as `CallToolRequest` → `INVALID_PARAMS` | `tools.rs` `handle_call_tool` | none | every `tools/call` test (all send a well-formed `{name, arguments}`) |
| Unknown tool name → `INVALID_PARAMS` ("Unknown tool: …") | `tools.rs` `handle_call_tool` | none | every `tools/call` test (all name a real tool) |
| Missing/non-string `query` argument | `tools.rs` `query_arg` | none | every SQL-tool test (all send a string `query`) |
| More than one statement in a single call | `tools.rs` `require_single_stmt` | `update_data_rejects_trailing_delete`, `insert_data_rejects_trailing_delete`, `delete_data_rejects_trailing_drop`, `schema_change_rejects_trailing_delete`, `execute_query_rejects_trailing_delete` | `update_data_accepts_single_update`, `update_data_allows_semicolon_inside_string` (a `;` *inside a string literal* must not be read as a second statement) |
| Empty query string ("No SQL statement provided") | `tools.rs` `require_single_stmt` | none | every SQL-tool test (all send non-empty SQL) |
| SQL that fails to parse ("Failed to parse SQL") | `tools.rs` `require_single_stmt` | none | every SQL-tool test (all send parseable SQL) |
| Statement of the wrong class for the tool (e.g. a `DELETE` sent to `insert_data`) | `tools.rs` `require_single_stmt` via `StmtClass::of` | **none** | `update_data_accepts_single_update` and the five `*_rejects_trailing_*` tests (each sends a matching-class first statement) |
| Write tools refused under `--readonly` | `tools.rs` `refuse_if_readonly` | `readonly_refuses_writes_and_still_serves_reads` (all four write tools) | same test (`current_database` still works), and every non-readonly test |
| A URI-shaped `path` bypassing `--readonly` (`file:...?mode=rwc`) | `tools.rs` `open_database` | `readonly_does_not_open_a_writable_connection_through_a_uri` | `the_catalog_marks_exactly_the_tools_that_are_refused` (`:memory:` still opens under `--readonly`) |
| Missing/non-string `path` argument | `tools.rs` `open_database` | none | `the_catalog_marks_exactly_the_tools_that_are_refused`, `readonly_does_not_open_a_writable_connection_through_a_uri` |
| Missing/non-string `table_name` argument | `tools.rs` `describe_table` | none | **none** — `describe_table` is never called from any test |
| Table not found ("Table '…' not found") | `tools.rs` `describe_table` | none | **none** — `describe_table` is never called from any test |

## HTTP transport — `turso-http-mcp/cli/http.rs` and `cli/mcp/http.rs`

| Guard | Where | Refuses bad input | Accepts good input |
|---|---|---|---|
| Request headers over 32 KiB, including when the terminator lands in the same read that crosses the cap | `http.rs` `read_http_request` | `oversized_headers_are_refused_even_when_the_terminator_arrives_with_them` (sized to 33 KiB, not padded far past the cap — see the ADR background on why size matters here) | `the_body_stops_at_the_declared_length`, `finds_header_end_across_read_boundaries`, and every live-server test |
| `Content-Length` that isn't a valid non-negative number | `http.rs` `parse_content_length` | `a_content_length_that_is_not_a_number_is_refused` (`abc`, empty, `-1`) | `content_length_repeated_with_one_value_is_allowed`, `the_body_stops_at_the_declared_length` |
| Duplicate `Content-Length` headers that disagree | `http.rs` `parse_content_length` | `content_length_headers_that_disagree_are_refused` | `content_length_repeated_with_one_value_is_allowed` (identical duplicates) — this is the pairing `background-agents.md` calls for explicitly |
| `Content-Length` that would overflow the request-end computation | `http.rs` `request_end` | `rejects_content_length_that_overflows` (`usize::MAX`) | same test (`(10, 5) -> 19`) |
| No header terminator found ("Invalid HTTP request") | `http.rs` `parse_http_request` | none | every live-server test (all well-formed) |
| Request line with fewer than two tokens ("Invalid request line") | `http.rs` `parse_http_request` | none | every live-server test |
| Empty request (no first line) ("Empty request") | `http.rs` `parse_http_request` | none | every live-server test |
| `Origin` present and not loopback → 403 | `mcp/http.rs` `http_response_for` / `forbidden_origin` | `a_request_with_a_forbidden_origin_is_rejected_before_the_body_is_even_read`; integration: `origin_validation_matches_the_documented_loopback_policy` (evil.example.com, `localhost.evil.com`, `127.0.0.1.attacker.net`, `null`) | same integration test (no `Origin`, `localhost`, `127.0.0.1`, `::1`, `127.0.0.2` all → 200) |
| Loopback check is real-host-parsing, not substring match | `mcp/http.rs` `origin_is_loopback` / `origin_host` | `a_hostname_that_merely_contains_a_loopback_name_is_not_loopback`, `the_null_origin_is_not_loopback` | `the_whole_127_block_is_loopback_not_just_127_0_0_1`, `origin_host_parses_the_authority_out_of_scheme_and_port` |
| Unknown path → 404 | `mcp/http.rs` `route_request` | `post_to_an_unknown_path_is_still_plain_not_found`, `get_to_an_unknown_path_is_plain_not_found_not_method_not_allowed`; integration: `a_request_to_an_unknown_path_returns_404` | `post_to_the_mcp_endpoint_is_handled` |
| `GET`/`DELETE` to `/mcp` → 405 | `mcp/http.rs` `route_request` | `get_to_the_mcp_endpoint_is_method_not_allowed`, `delete_to_the_mcp_endpoint_is_method_not_allowed` | `post_to_the_mcp_endpoint_is_handled` |
| Unimplemented JSON-RPC method → HTTP 404 | `mcp/http.rs` `http_status_for` | `a_post_naming_an_unimplemented_method_returns_404_with_a_method_not_found_error` | `a_post_of_tools_list_returns_200_with_the_json_rpc_result` |
| Unsupported protocol version → HTTP 400 | `mcp/http.rs` `http_status_for` | `a_request_naming_an_unsupported_protocol_version_returns_400` | `a_post_of_tools_list_returns_200_with_the_json_rpc_result` (no version claimed → 200); no HTTP-layer test sends a valid, explicit `PROTOCOL_V2` and asserts 200 — the shared check itself is covered in the protocol layer's own suite |
| v2 call missing `clientCapabilities` → HTTP 400 | `mcp/http.rs` `http_status_for` | `a_v2_tools_call_missing_client_capabilities_returns_400` | covered at the protocol layer (`a_v2_client_calls_a_tool_with_no_handshake_at_all`), not duplicated at the HTTP layer |
| Missing `Mcp-Method` header → `HEADER_MISMATCH` (400) | `mcp/http.rs` `validate_headers` | `a_request_missing_the_mcp_method_header_entirely_is_rejected`, `an_unparseable_body_with_no_mcp_method_header_is_still_rejected` | `a_post_of_tools_list_returns_200_with_the_json_rpc_result`, `two_identical_mcp_method_headers_are_accepted` |
| `Mcp-Method` header disagrees with body `method` | `mcp/http.rs` `validate_headers` | `an_mcp_method_header_that_disagrees_with_the_body_method_is_rejected` | every 200-status test (header matches body) |
| Duplicate `Mcp-Method`/`Mcp-Name` headers that disagree | `mcp/http.rs` `checked_header` | `two_disagreeing_mcp_method_headers_are_rejected` | `two_identical_mcp_method_headers_are_accepted` — again, the exact pairing `background-agents.md` asks for |
| Invalid characters (control bytes) in `Mcp-Method`/`Mcp-Name` | `mcp/http.rs` `is_valid_header_value` | `a_header_value_containing_invalid_characters_is_rejected`, `invalid_characters_in_mcp_method_itself_are_still_rejected` | `an_unrelated_header_with_non_ascii_bytes_is_accepted` — proves the check is scoped to the two headers this function reads, not every header on the request |
| Missing `Mcp-Name` on a name-required method | `mcp/http.rs` `validate_headers` | `a_tools_call_request_missing_the_mcp_name_header_is_rejected` | `a_plain_unencoded_mcp_name_matching_the_body_is_still_accepted` |
| `Mcp-Name` disagrees with `params.name`/`params.uri` | `mcp/http.rs` `validate_headers` | `an_mcp_name_header_that_disagrees_with_params_name_is_rejected` | `a_plain_unencoded_mcp_name_matching_the_body_is_still_accepted` |
| Base64-sentinel `Mcp-Name` that doesn't decode | `mcp/http.rs` `decode_base64_sentinel` | `a_base64_marked_mcp_name_with_an_undecodable_payload_is_rejected` | `a_base64_encoded_mcp_name_matching_the_body_is_accepted` |
| Base64-sentinel case sensitivity / mid-string false match | `mcp/http.rs` `decode_base64_sentinel` | `an_uppercase_base64_marker_is_treated_as_a_literal_value_not_decoded` (rejected, as it must compare literally and won't match) | `a_name_that_only_contains_the_sentinel_pattern_mid_string_is_treated_as_literal` (round-trips as a literal, reaches the tool) |

## What "none" does and doesn't mean

Every `none` above was checked by grepping the guard's error string or error
code across the whole test module (both the in-file `#[cfg(test)]` blocks and
`turso-http-mcp/cli/tests/`), not just the tests sitting next to the guard.
`turso-mcp-v2` has no `tests/integration/`-style directory for the MCP work —
the in-file modules in `mod.rs` and `tools.rs` are the entire suite for that
repo.

A blank "refuses bad input" cell is not evidence the guard is broken — most of
these are one-line presence or type checks unlikely to accept the very thing
they're built to reject. It is evidence the suite would not notice if they
were.
