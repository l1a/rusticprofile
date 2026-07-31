# Security Policy

## Reporting a Vulnerability

We take the security of `rusticprofile` seriously. If you believe you have found a security vulnerability, please do **not** open a public issue. Instead, please report it privately.

### Reporting Process

Please use GitHub's private security reporting via the "Report a vulnerability" button above.

*Note: Since this is a personal project, please allow for a few days for a response.*

### What to Expect

- You will receive an acknowledgment of your report within 48-72 hours.
- We will work with you to understand and validate the vulnerability.
- Once a fix is ready, we will coordinate a release and provide credit for your discovery if you wish.

### Particularly relevant classes

This tool orchestrates a backup program, so two categories matter more than they would elsewhere:

- **Secret disclosure.** Repository passwords and cloud credentials pass through the environment of the process rusticprofile spawns. Anything that causes them to reach a log file, a status file, an error message or a process listing is a security bug, not a cosmetic one.
- **Silent loss of coverage.** A defect that causes a backup to quietly cover less than it was configured to — or a retention rule to delete more than it should — is treated with the same seriousness as a disclosure bug.

Thank you for helping keep `rusticprofile` secure!
