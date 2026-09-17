# sluglib

## Spec: util.slug.slugify(text) -> str

- Lowercase the input.
- Every maximal run of characters outside [a-z0-9] becomes a single hyphen
  (non-ASCII characters count as outside).
- Strip leading and trailing hyphens.
- An input with no [a-z0-9] characters at all returns the empty string.
