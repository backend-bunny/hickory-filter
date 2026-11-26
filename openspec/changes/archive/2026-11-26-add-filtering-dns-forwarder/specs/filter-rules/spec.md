# Spec Delta

## Purpose

Decides whether a queried name is allowed or blocked, from allow and block rules given inline or loaded from lists, with allow always taking precedence.

## ADDED Requirements

### Requirement: Rule actions and types
A rule SHALL have an action, `allow` or `block`, and a type, `domain`, `wildcard` or `regex`, followed by its value. An inline rule SHALL be written as one line: `<action> <type> <value>`. A rule with an unknown action or type, or a missing value, SHALL be a configuration error.

#### Scenario: Inline rule accepted
- **WHEN** the configuration contains the rule `block domain ads.example`
- **THEN** the server starts and queries for `ads.example` are blocked

#### Scenario: Unknown type rejected
- **WHEN** the configuration contains the rule `block glob *.example`
- **THEN** startup fails with an error naming the rule and the accepted types

### Requirement: Name normalisation
Names SHALL be matched case-insensitively, with any trailing dot ignored, for both queries and rule values.

#### Scenario: Case and trailing dot
- **GIVEN** the rule `block domain Ads.Example.`
- **WHEN** a client queries `ADS.example`
- **THEN** the query is blocked

### Requirement: Domain rules match exactly
A `domain` rule SHALL match only the exact name it gives, and none of its subdomains.

#### Scenario: Exact name
- **GIVEN** the rule `block domain ads.example`
- **WHEN** a client queries `ads.example`
- **THEN** the query is blocked

#### Scenario: Subdomain not matched
- **GIVEN** the rule `block domain ads.example`
- **WHEN** a client queries `x.ads.example`
- **THEN** the query is not blocked by that rule

### Requirement: Wildcard rules match a name and its subdomains
A `wildcard` rule SHALL match its base name and every name below it, at label boundaries only.

#### Scenario: Base name and subdomain
- **GIVEN** the rule `block wildcard *.ads.example`
- **WHEN** a client queries `ads.example` or `a.b.ads.example`
- **THEN** both queries are blocked

#### Scenario: Label boundary
- **GIVEN** the rule `block wildcard *.ads.example`
- **WHEN** a client queries `badads.example`
- **THEN** the query is not blocked by that rule

### Requirement: Wildcard spellings
A wildcard value SHALL be accepted in any of these spellings, all meaning the same base name `x`: `x`, `*.x`, `.x`, `||x^`, `address=/x/` followed by any value (for example `address=/x/#` or `address=/x/0.0.0.0`). A value whose base name is not a valid DNS name SHALL be rejected.

#### Scenario: Equivalent spellings
- **WHEN** the rules `block wildcard ads.example`, `block wildcard *.ads.example`, `block wildcard ||ads.example^` and `block wildcard address=/ads.example/#` are each configured alone
- **THEN** each one blocks `ads.example` and `x.ads.example` the same way

#### Scenario: Invalid wildcard rejected inline
- **WHEN** the configuration contains `block wildcard ||exa mple^`
- **THEN** startup fails with an error naming the rule

### Requirement: Regex rules
A `regex` rule SHALL match when its expression matches anywhere in the normalised name (lower case, no trailing dot), unless the expression is anchored. Expressions SHALL use a syntax that runs in time linear in the input, with no backreferences or look-around. An expression that doesn't compile SHALL be a configuration error when inline.

#### Scenario: Unanchored match
- **GIVEN** the rule `block regex ^ad[0-9]+\.`
- **WHEN** a client queries `ad42.tracker.example`
- **THEN** the query is blocked

#### Scenario: Invalid inline regex
- **WHEN** the configuration contains `block regex (unclosed`
- **THEN** startup fails with an error naming the rule

### Requirement: Allow always wins
A name matched by any `allow` rule SHALL NOT be blocked, whatever `block` rules also match it and whichever source (inline or list) each rule came from.

#### Scenario: Allow inside a blocked wildcard
- **GIVEN** the rules `block wildcard *.ads.example` and `allow domain ok.ads.example`
- **WHEN** a client queries `ok.ads.example`
- **THEN** the query is forwarded upstream, not blocked

#### Scenario: Allow list overrides block regex
- **GIVEN** a block rule `block regex .*` and an allow list containing `intranet.example`
- **WHEN** a client queries `intranet.example`
- **THEN** the query is forwarded upstream

### Requirement: Unmatched names pass through
A name matched by no block rule SHALL be handled as if no filter existed.

#### Scenario: No match
- **GIVEN** only the rule `block domain ads.example`
- **WHEN** a client queries `news.example`
- **THEN** the query is forwarded upstream

### Requirement: List line syntax
List lines SHALL be read one rule per line, in the list's type. Blank lines and comments (from `#`, or a line starting with `!` in wildcard lists) SHALL be ignored. A `hosts` line SHALL be `<address> <name>`, with any IPv4 or IPv6 address and any whitespace between. Its address SHALL be ignored, and the line SHALL count as a `domain` rule for the name.

#### Scenario: Hosts lines with any address
- **GIVEN** a hosts list containing `0.0.0.0 a.example`, `127.0.0.1 b.example` and `::  c.example`
- **WHEN** clients query `a.example`, `b.example` and `c.example`
- **THEN** all three are blocked, and `x.a.example` is not

#### Scenario: Comments and blanks
- **GIVEN** a domain list containing `# header`, an empty line and `ads.example # inline comment`
- **WHEN** the list is loaded
- **THEN** it contributes exactly one rule, for `ads.example`

### Requirement: Unparseable list lines are skipped and counted
A list line that isn't valid for the list's type SHALL be skipped, never interpreted as a different rule. Wildcard lines carrying a `$` option suffix (for example `||x^$important`) SHALL be skipped. Each list SHALL count its skipped lines. A list SHALL NOT fail as a whole because some of its lines are invalid.

#### Scenario: Option suffix skipped
- **GIVEN** a wildcard list containing `||ads.example^$third-party` and `||track.example^`
- **WHEN** the list is loaded
- **THEN** `track.example` is blocked, `ads.example` is not, and the list reports one skipped line

#### Scenario: Invalid regex line skipped
- **GIVEN** a regex list containing `(unclosed` and `^ad\.`
- **WHEN** the list is loaded
- **THEN** `^ad\.` is in effect and the list reports one skipped line
