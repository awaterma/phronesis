# Claude Code transcript tail fixture (SYNTHETIC)

`interrupted-tail.jsonl` is **synthetic**. It was written by hand to the
line shape Claude Code's transcript JSONL uses (`type`, `message.role`,
`message.content` as a string or an array of `{type, text}` / `tool_use` /
`tool_result` blocks, `timestamp`, `sessionId`, `uuid`, `requestId`), with
every id, session, request, and model value replaced by an obviously fake
placeholder and every prose value replaced by `<redacted:N bytes>`.

It exists so `lifecycle_classify::the_committed_transcript_tail_is_recognized_as_an_interrupt`
exercises the interrupt-marker branch of `classify_prompt` against the exact
marker text Claude Code writes, `[Request interrupted by user]`, delivered as a
`user` entry whose content is an array of text blocks.

It is **not** a capture from a live session, and nothing here came from a real
transcript. A real tail, captured after pressing Esc in a scratch project and
redacted with `phr-mcp scrub-payload`, would be a stronger fixture; if one is
added, replace this file and update this README to say what it is and how it
was scrubbed. Never commit a transcript tail that has not been scrubbed of
prose, tool inputs, tool results, request ids, and thinking signatures.
