//! The census of traversal loops: counts over many programs, and nothing
//! else. A line names a class by the hash of its forms and gives the forms;
//! no program, shader hash, address, register or constant of a program is
//! in any of it.

use std::collections::BTreeMap;

use super::describe::{Class, Refusal};
use super::recognise::Report;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Census {
    /// Compute programs looked at, and those with a node instruction.
    pub programs: usize,
    pub with_node: usize,
    /// Compute programs that were not looked at because no IR was built of
    /// them: whether they hold a node instruction is not known, so they
    /// are in neither count above.
    pub unbuilt: usize,
    /// Node instructions, and those in no loop.
    pub node_instructions: usize,
    pub outside_loops: usize,
    /// Loops that hold a node instruction (a loop around another included).
    pub loops: usize,
    /// The recognised ones by class, with the class's forms.
    pub recognised: BTreeMap<Class, (usize, String)>,
    /// The others by reason.
    pub refused: BTreeMap<Refusal, usize>,
    /// The recogniser's work: instructions looked at, cases its tables
    /// were made over, and conditions evaluated in them
    /// ([`Report::visited`], [`Report::cases`], [`Report::evaluated`]).
    pub visited: usize,
    pub cases: usize,
    pub evaluated: usize,
}

impl Census {
    /// Counts one compute program.
    pub fn add(&mut self, report: &Report) {
        self.programs += 1;
        self.with_node += usize::from(report.node_instructions != 0);
        self.node_instructions += report.node_instructions;
        self.outside_loops += report.outside_loops;
        self.loops += report.loops.len();
        self.visited += report.visited;
        self.cases += report.cases;
        self.evaluated += report.evaluated;
        for finding in &report.loops {
            match &finding.result {
                Ok(d) => {
                    self.recognised
                        .entry(d.class)
                        .or_insert_with(|| (0, d.forms()))
                        .0 += 1
                }
                Err(why) => *self.refused.entry(*why).or_default() += 1,
            }
        }
    }

    /// Counts one compute program the IR builder refused: not examined.
    pub fn not_built(&mut self) {
        self.unbuilt += 1;
    }

    pub fn merge(&mut self, other: Census) {
        self.programs += other.programs;
        self.with_node += other.with_node;
        self.unbuilt += other.unbuilt;
        self.node_instructions += other.node_instructions;
        self.outside_loops += other.outside_loops;
        self.loops += other.loops;
        self.visited += other.visited;
        self.cases += other.cases;
        self.evaluated += other.evaluated;
        for (class, (n, forms)) in other.recognised {
            self.recognised.entry(class).or_insert((0, forms)).0 += n;
        }
        for (why, n) in other.refused {
            *self.refused.entry(why).or_default() += n;
        }
    }

    /// The census as lines: a summary and the recogniser's work, then a
    /// line per class and a line per reason, the most frequent first. Each
    /// begins with `ray tracing` or, indented, holds `recognised, class`
    /// or `refused:`.
    pub fn lines(&self) -> Vec<String> {
        let recognised: usize = self.recognised.values().map(|r| r.0).sum();
        let mut lines = vec![
            format!(
                "ray tracing  {} of {} compute programs examined hold a node instruction \
                 ({} instructions, {} in no loop); {} more compute programs were not built and not examined; \
                 {} loops hold one: {} recognised, {} refused",
                self.with_node,
                self.programs,
                self.node_instructions,
                self.outside_loops,
                self.unbuilt,
                self.loops,
                recognised,
                self.loops - recognised,
            ),
            format!(
                "ray tracing  the recogniser's work: {} instructions visited, {} conditions evaluated in {} cases",
                self.visited, self.evaluated, self.cases
            ),
        ];
        let mut classes: Vec<_> = self.recognised.iter().collect();
        classes.sort_by_key(|(class, (n, _))| (std::cmp::Reverse(*n), **class));
        for (class, (n, forms)) in classes {
            lines.push(format!("  {n:>7}  recognised, class {class}: {forms}"));
        }
        let mut refused: Vec<_> = self.refused.iter().collect();
        refused.sort_by_key(|(why, n)| (std::cmp::Reverse(**n), **why));
        for (why, n) in refused {
            lines.push(format!("  {n:>7}  refused: {}", why.reason()));
        }
        lines
    }
}
