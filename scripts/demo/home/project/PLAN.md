# Plan: search for the items API

Add a search route that finds items by title and ranks the newest first,
without slowing down the routes that exist.

## Route

- `GET /api/search?q=<text>` answers with the matching items as JSON.
- An empty query answers `400` instead of every item.
- At most 50 results; `&page=` reads further.

## Index

Keep a title index in memory and rebuild it when the store changes:

```rust
pub struct TitleIndex {
    words: HashMap<String, Vec<ItemId>>,
}
```

The index is rebuilt in full after every write.

## Tests

1. A prefix matches, case-insensitively.
2. Newer items come first.
3. An empty query is refused.

## Rollout

Ship behind the `search` feature in 0.5 and turn it on by default in 0.6.
