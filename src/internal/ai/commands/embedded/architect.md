---
name: architect
description: Analyze system design, evaluate trade-offs, and produce an ADR.
agent: architect
---

## /architect $ARGUMENTS

Analyze the architecture of the specified area and produce a design recommendation.

**Request:** $ARGUMENTS

### Design Process

1. **Understand the Problem** — What is being built or changed? What constraints exist?

2. **Survey Existing Architecture** — Use read_file and grep_files to understand:
   - Current module boundaries and dependencies
   - Patterns already in use
   - Public API surface

3. **Identify Options** — List 2-3 viable approaches with trade-offs (complexity, performance, maintainability).

4. **Recommend** — Choose the best option and justify the decision.

5. **Document** — Write an Architecture Decision Record (ADR):

```
# ADR: [Title]

## Status
Proposed

## Context
[Problem or requirement]

## Decision
[Chosen approach]

## Consequences
### Positive
- [benefit]

### Negative
- [trade-off]

### Risks
- [risk with mitigation]
```

**Principles:** Favor simplicity. Design for today's requirements. Keep module boundaries clean.
