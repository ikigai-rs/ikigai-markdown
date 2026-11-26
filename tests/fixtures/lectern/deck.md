<!-- slide: .center .middle -->
# Graphs from text

A short deck about lifting notes into a graph.

<!-- notes -->
Welcome everyone, and say what the hour covers.
<!-- /notes -->

---

## Triples

A triple is a subject, a predicate and an object.

```text
---
this separator is code, not a new slide
---
```

<!-- notes:presenter -->
Pause here for questions.
<!-- /notes -->

<!-- question
name: triple-parts
unit: rdf-basics
kind: choice
prompt: Which three parts make a triple?
choice: [x] subject, predicate, object
choice: [ ] subject, verb, object
choice: [ ] node, edge, label
-->

---

<!-- include: partials/basics.md#1-2, 4- -->
<!-- include: partials/basics.md -->

---

---

## Practice

<!-- notes -->
Hand out the exercise.
<!-- /notes -->

<!-- notes:presenter -->
Remind people that the check script runs locally.
<!-- /notes -->

<!-- exercise
name: load-a-graph
unit: rdf-basics
path: exercises/load-a-graph
title: Load a graph and count its triples
-->

<!-- question
name: count-triples
unit: rdf-basics
kind: exact
prompt: How many triples does the sample graph hold?
answer: 12
answer: twelve
-->

<!-- question
name: load-check
unit: rdf-basics
kind: check
prompt: Load the sample graph and print its size.
check: exercises/load-a-graph/check.sh
-->
