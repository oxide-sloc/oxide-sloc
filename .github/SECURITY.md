# Security Policy

## Supported Versions

| Version | Supported |
| ------- | --------- |
| Latest  | Yes       |

## Reporting a Vulnerability

Please **do not** open a public GitHub issue for security vulnerabilities.

Report vulnerabilities by emailing **nimzshafie@gmail.com** with the subject line
`[oxide-sloc] Security Vulnerability Report`.

Include:
- A description of the vulnerability and its impact
- Steps to reproduce
- Any suggested remediation if known

You can expect an acknowledgement within **72 hours** and a resolution or status update
within **14 days**.

## Scope

oxide-sloc is a local code-metrics tool. Its attack surface is limited to:
- Parsing arbitrary source files from the local filesystem
- Serving a web UI on localhost (port 4317 by default)
- Optional SMTP and webhook integrations (user-configured)

Known non-issues: analysing a crafted source file cannot execute code or escalate
privileges — the analyser is a read-only lexical state machine with no `eval`-equivalent
behaviour.
