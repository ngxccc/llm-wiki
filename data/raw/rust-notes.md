# Rust Notes

Important runtime details for LLM Wiki:
- Use `tokio` for async tasks
- Keep MCP logging on stderr only
- Use retry/backoff when the embedding backend is unavailable
- Keep `data/raw/` as the default knowledge source unless `LLM_WIKI_RAW_DATA_PATH` is set

This file exists to give the semantic search engine a few distinct keywords to retrieve.
