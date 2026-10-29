# Security policy

ToolLens sits between an AI agent and third-party MCP servers, so policy bypasses, secret leaks and
ways to make the gateway run a tool it should not are taken seriously.

## Reporting a vulnerability

Please report privately through GitHub's
[private vulnerability reporting](https://github.com/krishnashakula/toollens/security/advisories/new),
not in a public issue. Include a reproduction (a config and the calls that trigger it) if you can.

## In scope

- Running a tool that the configured policy denies, or that was never discovered or validated.
- Secrets reaching an upstream server, the model, the audit log or the console: the gateway's
  environment, server arguments or argument values.
- Reaching the console API without its token.
- Crashing or hanging the gateway from an upstream server's responses.

The design and its known limitations are in the README's
[Security model](README.md#security-model) and [Known limitations](README.md#known-limitations).
