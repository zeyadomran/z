---
name: minimal-functional-tests
description: Use whenever tests are needed for a feature or bug fix, when writing or changing automated tests, when reviewing test coverage, or when simplifying a test suite. Apply minimal functional coverage with one canonical test per flow and behavioral TDD.
---

# Minimal functional tests

Keep the smallest useful suite that verifies the app's base functionality and material failure behavior. Test each flow once through a public interface. This is the user's default testing strategy across projects; follow explicit task instructions and repository requirements where they differ.

## Choose coverage before editing

Read the repository's `AGENTS.md`, `docs/testing.md` if present, and the relevant existing tests. Use domain vocabulary from `CONTEXT.md` or applicable design decisions when available.

Identify the public service or module interface, the observable behavior being changed, and the existing test that owns that flow. Reuse established seams and coverage decisions without asking for repeated approval. Clarify only a material unresolved product or scope decision.

- Extend the canonical flow when its behavior changes. Avoid repeating it in unit, component, integration, and E2E suites.
- Add a separate case only for distinct logic or a material failure path that the canonical flow cannot express clearly. State what new behavior it protects.
- Keep each flow coherent; one test per flow does not mean combining unrelated capabilities into a giant test.
- Leave reversible cosmetic changes and trivial implementation details without new tests unless they change meaningful behavior or repository requirements demand coverage.
- Choose coverage by the consequence of a regression, not by line coverage targets, test counts, or hypothetical edge cases.

## Write functional tests

Exercise public behavior with real internal services and modules. A test may cross several internal modules; its value comes from the behavior it protects, not whether it is called a unit or integration test.

Use an owned temporary database when persistence matters. Verify durable behavior by closing and reopening it through the public service interface. Fake or inject only external boundaries that need control, such as OS APIs, network services, time, clipboard access, or an external failure.

Assert observable results, independently specified values, and important durable state. Avoid private-method assertions, internal collaborator mocks, incidental call counts, implementation-shaped snapshots, and expected values calculated with the production algorithm. When the public interface exposes the result, use it instead of inspecting internal database layout.

Control time and external completion explicitly for asynchronous behavior. Avoid arbitrary sleeps, tight timing thresholds, repeated flaky retries, or stress loops as substitutes for a deterministic behavioral check. Close owned resources and verify process shutdown before cleaning up test data.

## Use one TDD slice at a time

For new behavior or a bug fix:

1. Add or extend one behavioral assertion at the chosen public seam.
2. Run it and observe a failure caused by the missing behavior or bug. A setup, syntax, or environment error does not establish red.
3. Implement the smallest change that makes it pass, then run the focused test.
4. Continue with another slice only when it covers a necessary distinct behavior.

Keep structural refactoring separate from the initial red-to-green slice. Existing-suite consolidation is cleanup, not evidence that TDD occurred. Do not write a speculative batch of tests before implementing the first behavior.

## Simplify existing coverage

Map tests to the flows they protect. Choose one canonical owner for each duplicated flow, retain any unique useful assertion there, and remove redundant cases and helpers within the authorized scope. Preserve distinct failure handling, data integrity, and regression coverage. Fix a real behavioral failure instead of deleting its only protection to make CI green.

Use the existing runner and small fixtures. Do not introduce automated GUI/E2E suites, screenshot matrices, performance gates, stress suites, or custom test infrastructure unless explicitly requested. Preserve existing mandatory checks until their removal is authorized.

## Verify and report

Run the focused test during development, then the repository's required functional suite, type checks, lint, formatting, and applicable build checks. Read commands from current project configuration. Once checks pass, repeat them only after relevant changes or new evidence justifies it. Required CI must pass before an authorized merge.

Functional tests do not prove actual native shortcut delivery, clipboard fidelity, focus, accessibility, or installer behavior. When the task requires those outcomes, perform a bounded manual check using owned fixtures and record what was actually observed. For install/upgrade/uninstall checks, preserve existing user data and settings, verify retention and cleanup, and restore the baseline afterward. Do not turn manual qualification into another automated suite.

Report the canonical flow changed, validation performed, and material unverified behavior. A smaller test suite does not by itself resolve historical failures or qualify a release.
