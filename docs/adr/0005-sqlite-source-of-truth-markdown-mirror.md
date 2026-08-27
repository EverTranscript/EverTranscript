# SQLite is the source of truth; every Meeting auto-mirrors to Markdown

Meetings live in SQLite (with FTS) as the canonical store, and each Meeting is auto-mirrored to a Markdown file in an Operator-visible directory. SQLite gives search and structure; the Markdown mirror keeps the record legible, greppable, and usable without the app — the record is never hostage to the product.
