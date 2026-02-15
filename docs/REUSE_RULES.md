# Reuse Rules

## Mandatory rules
- Prefer existing open-source components over custom reimplementation.
- Before copying code, verify license compatibility.
- After copying/adapting code, update `CREDITS.md` immediately.
- Keep source links in code comments for non-trivial adapted snippets.

## Practical boundaries
- Reuse protocols and implementation patterns freely.
- Avoid large direct copies when a small adapter layer is enough.
- Keep local abstractions thin around third-party APIs to simplify upgrades.
