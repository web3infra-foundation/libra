---
name: plan
description: Create a structured implementation plan before coding.
agent: planner
---

## /plan $ARGUMENTS

Analyze the following request and create a structured implementation plan.

**Request:** $ARGUMENTS

### Planning Process

1. **Restate Requirements** — Clarify what needs to be built. Identify any ambiguities.

2. **Explore the Codebase** — Use read_file and grep_files to understand:
   - Existing relevant code and patterns
   - Module boundaries and dependencies
   - Test patterns in use

3. **Identify Risks**
   - Dependencies between components
   - Potential breaking changes
   - Areas of uncertainty
   - Complexity assessment (High/Medium/Low)

4. **Create Step-by-Step Plan**
   - Break down into phases
   - Each step should be specific and actionable
   - Include which files to modify/create
   - Note expected test changes

5. **Present Plan and Wait**

**CRITICAL:** After presenting the plan, STOP and wait for explicit user confirmation before proceeding with any implementation. The user must respond with an affirmative answer.

If the user wants changes, they will say:
- "modify: [changes]" — adjust the plan
- "different approach: [alternative]" — rethink the approach
- "proceed" / "go" / "yes" — begin implementation
