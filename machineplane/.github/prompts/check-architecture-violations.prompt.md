---
mode: 'agent'
description: 'Check architecture violations'
tools: ['changes', 'codebase', 'fetch', 'findTestFiles', 'githubRepo', 'problems', 'runCommands', 'runTasks', 'runTests', 'search', 'searchResults', 'testFailure', 'usages']
---

# 🔍 Check architecture violations

## 🎯 Objective
Analyze the codebase for the machineplane and the custom prompts in .github/prompts and validate them with the architecture under (./Readme.md or ./machineplane/Readme.md). Find any inconsistencies or missing documentation or invalid / contradicting implementions.

Highlight discrepancies and suggest changes for the specific files.

