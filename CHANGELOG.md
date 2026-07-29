# Changelog

All notable changes to runtrol will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Entries are written from the user's point of view. Internal code names, plan numbers,
and refactoring that no user can observe do not belong here.

## [Unreleased]

Design phase. No release yet. The repository holds the North Star, the architecture
decisions with their measured evidence, and the gate harness. No product code exists.

### Added

- North Star with a scored checklist. Every axis starts at 0 because nothing is built,
  and a score only counts when a gate actually runs in CI.
- Architecture decisions across eight initiatives, each recorded with the measurement
  or source reading that produced it.
- Contract gates that run before any product code exists: workspace hygiene, forbidden
  folder names, silent failure detection with a self test, and AI attribution blocking.
