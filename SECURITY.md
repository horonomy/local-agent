# Security Policy

## Supported Versions

Security fixes ship on the latest release line only. There are no maintenance
branches, and earlier minor versions do not receive backported fixes.

| Version                    | Supported          |
| -------------------------- | ------------------ |
| Latest released minor line | :white_check_mark: |
| Earlier minor lines        | :x:                |

For example, if the latest release is `0.8.6`, the supported minor line is `0.8.x`; `0.7.x` and older lines are unsupported.

Upgrade to the latest release before reporting. If the issue still reproduces
there, report it as described below.

## Reporting a Vulnerability

**Please do NOT open a public GitHub issue, pull request or RFC for security vulnerabilities, and do not post vulnerability details anywhere public.**

Report vulnerabilities privately through GitHub's private vulnerability reporting
for this repository: [open a private advisory](https://github.com/horonomy/local-agent/security/advisories/new).
No response-time commitment is published yet.

Local Agent is derived from ZeroClaw. The upstream project's security policy
covers upstream releases only; do not use it to report a Local Agent issue.

### What to Include

When a channel exists, a report should have:

- Description of the vulnerability
- Steps to reproduce
- Impact assessment
- Suggested fix (if any)

## Security Architecture

ZeroClaw implements defense-in-depth security:

### Autonomy Levels
- **ReadOnly** — Agent can only read, no shell or write access
- **Supervised** — Agent can act within allowlists (default)
- **Full** — Agent has full access within workspace sandbox

### Sandboxing Layers
1. **Workspace isolation** — All file operations confined to workspace directory
2. **Path traversal blocking** — `..` sequences and absolute paths rejected
3. **Command allowlisting** — Only explicitly approved commands can execute
4. **Forbidden path list** — Critical system paths (`/etc`, `/root`, `~/.ssh`) always blocked
5. **Rate limiting** — Max actions per hour and cost per day caps

### What We Protect Against
- Path traversal attacks (`../../../etc/passwd`)
- Command injection (`rm -rf /`, `curl | sh`)
- Workspace escape via symlinks or absolute paths
- Runaway cost from LLM API calls
- Unauthorized shell command execution

## Security Testing

All security mechanisms are covered by automated tests (129 tests):

```bash
cargo test -- security
cargo test -- tools::shell
cargo test -- tools::file_read
cargo test -- tools::file_write
```

## Container Security

ZeroClaw Docker images follow CIS Docker Benchmark best practices:

| Control | Implementation |
|---------|----------------|
| **4.1 Non-root user** | Container runs as UID 65534 (distroless nonroot) |
| **4.2 Minimal base image** | `gcr.io/distroless/cc-debian13:nonroot` — no shell, no package manager |
| **4.6 HEALTHCHECK** | Not applicable (stateless CLI/gateway) |
| **5.25 Read-only filesystem** | Supported via `docker run --read-only` with `/workspace` volume |

### Verifying Container Security

```bash
# Build and verify non-root user
docker build -t zeroclaw .
docker inspect --format='{{.Config.User}}' zeroclaw
# Expected: 65534:65534

# Run with read-only filesystem (production hardening)
docker run --read-only -v /path/to/workspace:/workspace zeroclaw gateway
```

### CI Enforcement

The `source-images` job in `.github/workflows/docker-image-pr.yml` verifies that its loaded default and Alpine `linux/amd64` images are configured to run as `65534:65534`.
