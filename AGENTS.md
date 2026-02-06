# Agent Instructions

## General

- When a bug was identified, always add a test to confirm the issue. Then fix the bug, and finally run the same test again to confirm that the issue is gone. Keep the test to ensure that the bug will not happen anymore

## Project-specific

- Before implementing XFA functionality, always consult the [XFA specs](./specs/XFA-3_3.md)
- When checking bounds in a module, consider adding a helper method to Bounds directly or check if a helper function already exists
- 