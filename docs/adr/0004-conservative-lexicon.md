# ADR 0004: Lexicon corrections are exact and reversible

**Status:** Accepted

The lexicon supplies bounded prompt hints and makes only exact,
Unicode-word-boundary replacements for explicit variants. Fuzzy matching and
regular-expression rules are excluded from v1. Each applied replacement has a
stored rule and source location, allowing an individual revert.
