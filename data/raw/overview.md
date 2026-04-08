# LLM Wiki Overview

LLM Wiki is a local-first PKM system built with an event-driven RAG pipeline.

Key points:
- Watch markdown files from `data/raw/`
- Chunk text and embed it through an HTTP embedding API
- Store vectors in Qdrant
- Expose `search_wiki` over MCP stdio

This sample file is intentionally short so you can verify search results quickly.
