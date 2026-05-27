# Agent Instructions

## General

- When a bug was identified, always add a test to confirm the issue. Then fix the bug, and finally run the same test again to confirm that the issue is gone. Keep the test to ensure that the bug will not happen anymore.
- After implementing a feature, always run all tests to ensure that the feature din't introduce a bug.
- Before assuming that a test failure was preexisting, confirm it by stashing the changes and running the tests again in the original code.
- In case you introduced a bug, try to identify the issue by comparing with the previous working state. Based on that analysis, fix the issue on the new code.
- Instead of assuming something when the task is unclear, ask the user.
- When making an architectural decision, always ask the user first with all possible options.
- When asked to find the issue of a failing test, do not simplify the test itself without the users consent.
- As soon as you have implemented a feature and all tests pass, commit it. Use a simple one-sentence commit message.
- After finishing the implementation, review the code from a separate subagent that only knows the plan but not the actual implementation. The subagent should review the code (cleanliness, code duplication, potential bugs or missing implementation details, run clippy lints, removed or changed tests ...) and provide feedback to the main agent.

## Project-specific

- When running tests, use `cargo test --release` such that they are faster. Also make sure to save the entire output to a log file to avoid needing to re-run the tests if output information is required.
- Before implementing XFA functionality, always consult the [XFA specs](./specs/XFA-3_3.md).
- When checking bounds in a module, consider adding a helper method to Bounds directly or check if a helper function already exists.
- Tests should never work with the ouput files. Instead they should analyze the intermediate structures (StrucuredNode, FlattenedNode, Document, ...) directly.