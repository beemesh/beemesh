---
mode: 'agent'
description: 'Check DRY'
tools: ['changes', 'codebase', 'fetch', 'findTestFiles', 'githubRepo', 'problems', 'edit', 'runCommands', 'runTasks', 'runTests', 'search', 'searchResults', 'testFailure', 'usages']
---

# 🔍 DRY Violation Analysis & Refactoring Recommendations

## 🎯 Objective
Analyze the codebase for DRY (Don't Repeat Yourself) violations and provide actionable refactoring recommendations to improve code maintainability, reduce duplication, and enhance overall code quality.

## 🔎 Areas to Analyze

### 1. **API Route Patterns** 🛣️
Examine all behaviour and control files in `machine/src/libp2p_beemesh/` for:
- **Crypto**: all crypto relevant support functions should be in crypto
- **protocol**: message definitions in flatbuffer or other formats, constants and protocol route definition in libp2p that are strings and are not part of the protocol crate
- **scheduler**: scheduling logic and related helper functions
- **Error Handling**: Duplicate error responses
- **Validation Logic**: Similar input validation patterns, every envelope must be validated and there are no fallbacks to unsigned envelopes
- **Response Formatting**: Repeated cloning and formatting of response structures

## 📊 Analysis Framework

### **Severity Levels**
- 🔴 **Critical**: Extensive duplication (>5 instances)
- 🟡 **Moderate**: Notable duplication (3-5 instances)
- 🟢 **Minor**: Limited duplication (2-3 instances)

### **Refactoring Impact**
- ⚡ **High Impact**: Affects multiple files/modules
- 📈 **Medium Impact**: Affects single module/feature
- 🔧 **Low Impact**: Localized improvements

## 📝 Deliverables

### **1. Violation Report**
Create a detailed report containing:
- **File-by-file analysis** of duplication found
- **Code snippets** showing exact duplicated patterns
- **Severity assessment** using the framework above
- **Quantified metrics** (lines duplicated, files affected)

### **2. Refactoring Plan**
Provide a prioritized action plan with:
- **Quick Wins**: Easy refactoring opportunities
- **Medium Efforts**: Moderate refactoring tasks
- **Large Projects**: Comprehensive restructuring

### **3. Implementation Suggestions**
For each violation, provide:
- **Before/After Code Examples**: Show current vs. refactored code
- **Migration Steps**: Step-by-step refactoring instructions
- **Testing Strategy**: How to validate refactoring doesn't break functionality
- **Performance Impact**: Expected improvements in bundle size, maintainability

## 🎯 Success Criteria

- [ ] **Comprehensive Analysis**: All major code duplication identified
- [ ] **Actionable Recommendations**: Clear, implementable suggestions
- [ ] **Prioritized Backlog**: Tasks ranked by impact vs. effort
- [ ] **Code Examples**: Concrete before/after demonstrations
- [ ] **Metrics**: Quantified improvement estimates

## 🚀 Getting Started

1. **Scan libp2p behaviours and control Files**: Start with `machine/src/libp2p_beemesh/` directory
2. **Analyze Patterns**: Look for identical or nearly identical code blocks
3. **Document Findings**: Create violation inventory with severity ratings
4. **Propose Solutions**: Design generic abstractions and patterns
5. **Create Examples**: Show practical refactoring implementations