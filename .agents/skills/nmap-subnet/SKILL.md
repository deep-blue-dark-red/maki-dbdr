---
name: nmap-subnet
description: Scan the full range of the local subnet using nmap discovery mode to enumerate live hosts
---

# nmap-subnet

## Use this skill when
- You need to discover live hosts on the local subnet
- You need to quickly inventory devices on a /24 network
- A network connectivity or device enumeration task is requested

## Instructions
1. Run `nmap -sn <local_subnet>/24` where `<local_subnet>` is the /24 network prefix (e.g., `192.168.1` for `192.168.1.0/24`).
2. Parse the output to identify live hosts (lines containing "Nmap scan reports for" followed by "host appears to be up").
3. Report the list of discovered IP addresses and any hostnames resolved.
4. If nmap is not installed or permission is denied, inform the user and suggest installing nmap or running with elevated privileges.
