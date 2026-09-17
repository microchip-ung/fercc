# Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
# SPDX-License-Identifier: MIT

# Security Policy

## Reporting a Vulnerability

**Please do not report security vulnerabilities through public GitHub issues,
pull requests, or any other public forum.**

Suspected security vulnerabilities in fercc should be reported to Microchip's
Product Security Incident Response Team (PSIRT):

- <https://www.microchip.com/psirt>

Responsible disclosure gives us the opportunity to investigate and address the
issue before it is made public.

## Dependencies

fercc's Rust dependency tree is tracked via `Cargo.lock`. A software bill of
materials (SBOM) in SPDX format is published alongside each release and
can be used to cross-reference dependencies against vulnerability databases.

## Vulnerability monitoring

Microchip runs a periodic CI job (see `Jenkinsfile_UNGE` in this repository)
that generates an SBOM from every build and scans it against public
vulnerability databases using [Grype](https://github.com/anchore/grype). The
job runs at minimum daily on the main branch.

When a vulnerability is identified the responsible engineering team is notified
automatically. Issues are triaged and addressed in prioritised order alongside
other engineering work. **We cannot guarantee a specific response time or
remediation schedule.**

If your use of fercc requires stronger security guarantees than the above, we
encourage you to:

- Run your own SBOM scan against the published `fercc-sbom.spdx.json` and/or
  `Cargo.lock` using a tool which matches your requirements.
- Establish your own alerting and remediation pipeline so you can act on new
  findings according to your own risk tolerance and timelines.

