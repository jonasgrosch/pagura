//! RFC 9618 5: one node per policy per depth, with shared parent edges.
use super::budget::Budget;
use super::{Check, InvalidReason, Issue, Oid, ValidationOptions};
use std::collections::{BTreeMap, BTreeSet};

// Interned IDs keep graph operations independent of attacker-chosen OID lengths.
const ANY: usize = 0;
// Covers even a sparsely occupied B-tree leaf, on native and WASM targets.
const TREE_ENTRY: usize = 512;
struct Node {
    policy: usize,
    expected: BTreeSet<usize>,
    parents: Vec<usize>,
    depth: usize,
    alive: bool,
}
pub(super) struct Graph {
    identifiers: Vec<Oid>,
    lookup: BTreeMap<Oid, usize>,
    nodes: Vec<Node>,
    current: Vec<usize>,
    pub explicit: usize,
    pub mapping: usize,
    pub any: usize,
}

impl Graph {
    pub(super) fn new(
        n: usize,
        options: &ValidationOptions<'_>,
        budget: &mut Budget<'_>,
    ) -> Check<Self> {
        let mut graph = Self {
            identifiers: Vec::new(),
            lookup: BTreeMap::new(),
            nodes: Vec::new(),
            current: Vec::new(),
            explicit: if options.explicit_policy { 0 } else { n + 1 },
            mapping: if options.inhibit_mapping { 0 } else { n + 1 },
            any: if options.inhibit_any_policy { 0 } else { n + 1 },
        };
        graph.intern(&Oid::any(), budget)?;
        let root = graph.add(ANY, BTreeSet::from([ANY]), Vec::new(), 0, budget)?;
        graph.current.push(root);
        Ok(graph)
    }
    fn intern(&mut self, oid: &Oid, budget: &mut Budget<'_>) -> Check<usize> {
        let comparisons = usize::try_from(self.lookup.len().checked_ilog2().unwrap_or(0) + 2)
            .unwrap_or(usize::MAX);
        budget.work(oid.0.len().saturating_add(1).saturating_mul(comparisons))?;
        if let Some(&i) = self.lookup.get(oid) {
            return Ok(i);
        }
        budget.node()?;
        budget.bytes(TREE_ENTRY + oid.0.len().saturating_mul(2))?;
        let i = self.identifiers.len();
        self.identifiers.push(oid.clone());
        self.lookup.insert(oid.clone(), i);
        Ok(i)
    }
    fn add(
        &mut self,
        policy: usize,
        expected: BTreeSet<usize>,
        parents: Vec<usize>,
        depth: usize,
        budget: &mut Budget<'_>,
    ) -> Check<usize> {
        budget.node()?;
        budget.bytes(TREE_ENTRY)?;
        budget.nodes(expected.len().saturating_add(parents.len()))?;
        budget.bytes(
            expected
                .len()
                .saturating_mul(TREE_ENTRY)
                .saturating_add(parents.len().saturating_mul(16)),
        )?;
        let i = self.nodes.len();
        self.nodes.push(Node {
            policy,
            expected,
            parents,
            depth,
            alive: true,
        });
        Ok(i)
    }
    fn parents(
        &self,
        previous: &[usize],
        policy: usize,
        budget: &mut Budget<'_>,
    ) -> Check<Vec<usize>> {
        let mut parents = Vec::new();
        for &i in previous {
            budget.work(1 + self.nodes[i].expected.len())?;
            if self.nodes[i].expected.contains(&policy) {
                budget.bytes(16)?;
                parents.push(i);
            }
        }
        if parents.is_empty() {
            for &i in previous {
                budget.work(1)?;
                if self.nodes[i].policy == ANY {
                    budget.bytes(16)?;
                    parents.push(i);
                }
            }
        }
        Ok(parents)
    }
    pub(super) fn process(
        &mut self,
        policies: Option<&[Oid]>,
        depth: usize,
        self_issued: bool,
        final_cert: bool,
        budget: &mut Budget<'_>,
    ) -> Check<()> {
        let previous = std::mem::take(&mut self.current);
        if let Some(policies) = policies.filter(|_| !previous.is_empty()) {
            for oid in policies.iter().filter(|p| **p != Oid::any()) {
                let policy = self.intern(oid, budget)?;
                let parents = self.parents(&previous, policy, budget)?;
                if !parents.is_empty() {
                    let i = self.add(policy, BTreeSet::from([policy]), parents, depth, budget)?;
                    self.current.push(i);
                }
            }
            if policies.contains(&Oid::any()) && (self.any > 0 || (self_issued && !final_cert)) {
                self.expand_any(&previous, depth, budget)?;
            }
        }
        self.prune(budget)?;
        self.required()
    }
    fn expand_any(
        &mut self,
        previous: &[usize],
        depth: usize,
        budget: &mut Budget<'_>,
    ) -> Check<()> {
        let mut expected = BTreeMap::<usize, Vec<usize>>::new();
        for &i in previous {
            for &p in &self.nodes[i].expected {
                budget.work(1 + self.nodes[i].expected.len())?;
                budget.bytes(TREE_ENTRY)?;
                expected.entry(p).or_default().push(i);
            }
        }
        for (policy, parents) in expected {
            budget.work(self.current.len())?;
            if !self.current.iter().any(|&i| self.nodes[i].policy == policy) {
                let i = self.add(policy, BTreeSet::from([policy]), parents, depth, budget)?;
                self.current.push(i);
            }
        }
        Ok(())
    }
    fn prune(&mut self, budget: &mut Budget<'_>) -> Check<()> {
        budget.bytes(self.nodes.len())?;
        let mut keep = vec![false; self.nodes.len()];
        for &i in &self.current {
            keep[i] = true;
        }
        for i in (0..self.nodes.len()).rev() {
            budget.work(1)?;
            if keep[i] {
                for &parent in &self.nodes[i].parents {
                    budget.work(1)?;
                    keep[parent] = true;
                }
            }
            self.nodes[i].alive &= keep[i];
        }
        Ok(())
    }
    pub(super) fn mappings(
        &mut self,
        mappings: &[(Oid, Oid)],
        depth: usize,
        budget: &mut Budget<'_>,
    ) -> Check<()> {
        let mut grouped = BTreeMap::<usize, BTreeSet<usize>>::new();
        for (issuer, subject) in mappings {
            let issuer = self.intern(issuer, budget)?;
            let subject = self.intern(subject, budget)?;
            if issuer == ANY || subject == ANY {
                return Err(Issue::Invalid(InvalidReason::AnyPolicyMapping));
            }
            budget.work(1 + mappings.len())?;
            budget.bytes(TREE_ENTRY)?;
            grouped.entry(issuer).or_default().insert(subject);
        }
        for (issuer, subjects) in grouped {
            budget.work(self.current.len())?;
            let node = self
                .current
                .iter()
                .copied()
                .find(|&i| self.nodes[i].policy == issuer);
            if self.mapping == 0 {
                if let Some(i) = node {
                    self.current.retain(|&n| n != i);
                }
            } else if let Some(i) = node {
                budget.nodes(subjects.len())?;
                self.nodes[i].expected = subjects;
            } else if let Some(any) = self
                .current
                .iter()
                .copied()
                .find(|&i| self.nodes[i].policy == ANY)
            {
                budget.bytes(self.nodes[any].parents.len().saturating_mul(16))?;
                let parents = self.nodes[any]
                    .parents
                    .iter()
                    .copied()
                    .filter(|&i| self.nodes[i].policy == ANY)
                    .collect();
                let i = self.add(issuer, subjects, parents, depth, budget)?;
                self.current.push(i);
            }
        }
        self.prune(budget)
    }
    pub(super) fn required(&self) -> Check<()> {
        if self.explicit == 0 && self.current.is_empty() {
            return Err(Issue::Invalid(InvalidReason::Policy));
        }
        Ok(())
    }
    pub(super) fn decrement(&mut self) {
        self.explicit = self.explicit.saturating_sub(1);
        self.mapping = self.mapping.saturating_sub(1);
        self.any = self.any.saturating_sub(1);
    }
    pub(super) fn finish(
        &self,
        initial: &[Oid],
        depth: usize,
        budget: &mut Budget<'_>,
    ) -> Check<Vec<Oid>> {
        let mut authority = BTreeSet::new();
        for node in self.nodes.iter().filter(|node| node.alive) {
            budget.work(1)?;
            if (node.policy == ANY && node.depth == depth)
                || (node.policy != ANY
                    && node.parents.len() == 1
                    && self.nodes[node.parents[0]].policy == ANY)
            {
                budget.bytes(TREE_ENTRY + self.identifiers[node.policy].0.len())?;
                authority.insert(self.identifiers[node.policy].clone());
            }
        }
        for oid in initial {
            budget.work(
                oid.0
                    .len()
                    .saturating_add(1)
                    .saturating_mul(authority.len().saturating_add(1)),
            )?;
            budget.bytes(TREE_ENTRY + oid.0.len())?;
        }
        let result = if initial.contains(&Oid::any()) {
            authority
        } else if authority.contains(&Oid::any()) {
            initial.iter().cloned().collect()
        } else {
            authority
                .into_iter()
                .filter(|p| initial.contains(p))
                .collect()
        };
        if self.explicit == 0 && result.is_empty() {
            return Err(Issue::Invalid(InvalidReason::Policy));
        }
        Ok(result.into_iter().collect())
    }
}
