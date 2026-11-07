# Security Policy

Zenith is a security-first daemon designed to defend downstream Minecraft infrastructures against denial-of-service, slowloris stalls, and malformed packet attacks.

## Supported Versions

| Version | Protocol | Supported |
|---|:---:|:---:|
| `0.1.x` (main) | Minecraft 26.3 / Protocol 777 | :white_check_mark: |

## Reporting a Vulnerability

If you discover a potential security vulnerability in Zenith (e.g. denial of service, memory exhaustion, HMAC tampering, or bypass of connection limiters):

1. **Private Disclosure**: Please report it privately via GitHub's [Private Vulnerability Advisory](https://github.com/xxaxio/Zenith/security/advisories/new).
2. **Response SLA**: The maintainer will acknowledge receipt within **48 hours** and provide an initial assessment or remediation timeline.
3. **No Public Issues**: Please do **not** disclose security-critical vulnerabilities through public GitHub issues, PRs, or discussions until a patch has been released.
