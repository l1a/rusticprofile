## Description

Please include a summary of the change and which issue is fixed. Please also include relevant motivation and context.

Fixes # (issue)

## Type of change

- [ ] Bug fix (non-breaking change which fixes an issue)
- [ ] New feature (non-breaking change which adds functionality)
- [ ] Breaking change (fix or feature that would cause existing functionality to not work as expected)
- [ ] Design decision (updates `PLAN.md`)
- [ ] This change requires a documentation update

## How Has This Been Tested?

Please describe the tests that you ran to verify your changes.

- [ ] Test A
- [ ] Test B

## Backup safety

This project drives a tool that writes to, and deletes from, real backup repositories.

- [ ] No `prune` was run against a shared repository
- [ ] No snapshots were deleted without explicit per-step authorisation
- [ ] Every write test went to a throwaway repository, not a production one

## Checklist:

- [ ] My code follows the style guidelines of this project
- [ ] I have performed a self-review of my own code
- [ ] I have commented my code, particularly in hard-to-understand areas
- [ ] I have made corresponding changes to the documentation
- [ ] If the package version was bumped, I ran `just man` and committed the regenerated `docs/rusticprofile.1`
- [ ] My changes generate no new warnings
- [ ] I have added tests that prove my fix is effective or that my feature works
- [ ] New and existing unit tests pass locally with my changes
- [ ] No failure mode I introduced can degrade silently — it errors, or it is reported
