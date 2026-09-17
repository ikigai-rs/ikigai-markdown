# Cache every lift

## Record

- **Status**: Accepted
- **Date**: 2026-02-03

## Context

A lift is a pure function of the document and the mapping, as DR-0001 made the
mapping a resource. Recomputing it on every read wastes the work.

## Decision

Cache the lift under both inputs' golden threads. The measurements live in DR-0007.
