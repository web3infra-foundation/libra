---
name: tdd
description: Guide test-driven development with RED-GREEN-REFACTOR cycle.
agent:
---

## /tdd $ARGUMENTS

Guide a test-driven development workflow for the specified feature.

**Feature:** $ARGUMENTS

### TDD Cycle: RED → GREEN → REFACTOR

#### Phase 1: RED (Write Failing Tests)

1. **Define Types/Interfaces** — Scaffold the public API surface first
2. **Write Tests** — Write tests that exercise the expected behavior
3. **Run Tests** — Verify tests FAIL (this is required)

```
cargo test -- <test_name>
```

The tests MUST fail before proceeding. If they pass, something is wrong.

#### Phase 2: GREEN (Minimal Implementation)

1. **Implement minimum code** to make the failing tests pass
2. **No extra features** — Only write what's needed to pass
3. **Run tests** — Verify all tests pass

#### Phase 3: REFACTOR

1. **Improve code quality** while keeping tests green
2. **Extract helpers** if there's duplication
3. **Improve naming** for clarity
4. **Run tests** — Verify they still pass after refactoring

### Test Categories

Write tests for each category as applicable:

- **Happy path** — Normal expected behavior
- **Edge cases** — Empty inputs, boundary values, max/min
- **Error conditions** — Invalid inputs, missing resources
- **Concurrency** — Race conditions, async ordering (if applicable)

### Coverage Requirements

- **80% minimum** for all new code
- **100% required** for:
  - Error handling paths
  - Security-critical code
  - Core business logic

### Critical Rules

1. **NEVER skip the RED phase** — Tests must fail before implementation
2. **One test at a time** — Write one test, make it pass, repeat
3. **No premature optimization** — Get it working first, optimize later
4. **Test behavior, not implementation** — Tests should survive refactoring
