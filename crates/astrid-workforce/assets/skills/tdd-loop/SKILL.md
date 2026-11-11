---
name: tdd-loop
description: The red-green-refactor cycle applied honestly, with tests that pin behavior instead of implementation details.
when_to_use: Use when implementing a new function, fixing a bug with a known shape, or when a task says "test-driven" or the change touches critical logic.
---

# TDD loop

1. Red: write one small failing test that names the behavior you want, not the method you plan to add. Run it and watch it fail for the right reason.
2. Green: write the minimum code that makes the test pass. Resist the urge to handle cases no test asked for yet.
3. Refactor: with the test green, improve names, remove duplication, and simplify. Run the test after every move; refactor steps stay green.
4. Repeat with the next behavior. One test per cycle keeps failures cheap to interpret.

Rules that keep the loop honest:

- A test that never failed is not a test; delete it or break the code to watch it fail.
- Test behavior through the public surface wherever possible; mocks should model real contracts, not wishful interfaces.
- Bug fixes start with a failing test that reproduces the bug, then the fix — the test becomes the permanent regression guard.
- If you cannot write the test, the design is wrong: split the unit until behavior can be isolated.
