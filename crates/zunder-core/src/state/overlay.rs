//! Copy-on-write view of a [`State`] for push validation.
//!
//! Validation applies the candidate operations of a push on top of the
//! current state and checks the result. Copying the whole state for that
//! costs time proportional to the database, not to the push. The overlay
//! copies only the entities the push touches (and the child sets of parents
//! whose children change); everything else is read from the base state. The
//! set of known op IDs is never copied.

use std::collections::{BTreeSet, HashMap};

use uuid::Uuid;

use super::{Entity, FieldEntry, Lookup, State};
use crate::op::{EntityKind, Op};

type Key = (EntityKind, Uuid);

pub(crate) struct Overlay<'a> {
    base: &'a State,
    /// Touched entities after applying the push (base copy plus operations).
    entities: HashMap<Key, Entity>,
    /// Child sets that differ from the base; they replace the base set.
    children: HashMap<Uuid, BTreeSet<Uuid>>,
}

impl<'a> Overlay<'a> {
    pub(crate) fn new(base: &'a State) -> Self {
        Overlay {
            base,
            entities: HashMap::new(),
            children: HashMap::new(),
        }
    }

    /// Merges one operation into the overlay, like [`State::apply`].
    pub(crate) fn apply(&mut self, op: &Op) {
        let key = (op.entity, op.entity_id);
        let base = self.base;
        let entity = self.entities.entry(key).or_insert_with(|| {
            base.entities
                .get(&key)
                .cloned()
                .unwrap_or_else(|| Entity::new(op.entity, op.entity_id, op.hlc.clone()))
        });
        let entry = FieldEntry {
            value: op.value.clone(),
            hlc: op.hlc.clone(),
            op_id: op.op_id,
        };
        if let Some((old, new)) = entity.merge(&op.field, entry, &op.hlc).parent_move {
            self.reparent(op.entity_id, old, new);
        }
    }

    /// Drops every operation applied to `key`: the entity reads as in the
    /// base state again.
    pub(crate) fn revert(&mut self, key: Key) {
        let Some(entity) = self.entities.remove(&key) else {
            return;
        };
        if key.0 == EntityKind::Task {
            let applied = entity.parent_id();
            let original = self
                .base
                .entity(EntityKind::Task, key.1)
                .and_then(Entity::parent_id);
            if applied != original {
                self.reparent(key.1, applied, original);
            }
        }
    }

    /// All entities of one kind (including tombstoned ones), in no
    /// particular order.
    pub(crate) fn entities_of(&self, kind: EntityKind) -> impl Iterator<Item = &Entity> + '_ {
        let changed_or_base = self
            .base
            .entities_of(kind)
            .map(move |e| self.entities.get(&(kind, e.id())).unwrap_or(e));
        let created = self
            .entities
            .iter()
            .filter(move |(key, _)| key.0 == kind && !self.base.entities.contains_key(key))
            .map(|(_, e)| e);
        changed_or_base.chain(created)
    }

    /// Number of entities copied into the overlay.
    #[cfg(test)]
    pub(crate) fn copied(&self) -> usize {
        self.entities.len()
    }

    fn reparent(&mut self, child: Uuid, old: Option<Uuid>, new: Option<Uuid>) {
        if let Some(old) = old {
            self.child_set_mut(old).remove(&child);
        }
        if let Some(new) = new {
            self.child_set_mut(new).insert(child);
        }
    }

    fn child_set_mut(&mut self, parent: Uuid) -> &mut BTreeSet<Uuid> {
        let base = self.base;
        self.children
            .entry(parent)
            .or_insert_with(|| base.children.get(&parent).cloned().unwrap_or_default())
    }
}

impl Lookup for Overlay<'_> {
    fn entity(&self, kind: EntityKind, id: Uuid) -> Option<&Entity> {
        self.entities
            .get(&(kind, id))
            .or_else(|| self.base.entities.get(&(kind, id)))
    }

    fn child_set(&self, parent: Uuid) -> Option<&BTreeSet<Uuid>> {
        self.children
            .get(&parent)
            .or_else(|| self.base.children.get(&parent))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{h, op, uid};
    use serde_json::json;

    const T: EntityKind = EntityKind::Task;

    fn base() -> State {
        State::from_ops(&[
            op(1, T, 10, "title", json!("Eltern"), h(1, 0, "a")),
            op(2, T, 11, "title", json!("Kind"), h(1, 1, "a")),
            op(
                3,
                T,
                11,
                "parent_id",
                json!(uid(10).to_string()),
                h(1, 2, "a"),
            ),
            op(4, T, 12, "title", json!("Andere"), h(1, 3, "a")),
        ])
    }

    /// Applying a list of operations to the overlay reads exactly like
    /// applying them to a full copy of the state.
    #[test]
    fn reads_like_a_full_copy() {
        let base = base();
        let ops = [
            op(
                5,
                T,
                11,
                "parent_id",
                json!(uid(12).to_string()),
                h(2, 0, "b"),
            ),
            op(6, T, 13, "title", json!("Neu"), h(2, 1, "b")),
            op(
                7,
                T,
                13,
                "parent_id",
                json!(uid(11).to_string()),
                h(2, 2, "b"),
            ),
            op(8, T, 10, "deleted", json!(true), h(2, 3, "b")),
        ];
        let mut overlay = Overlay::new(&base);
        let mut copy = base.clone();
        for o in &ops {
            overlay.apply(o);
            copy.apply(o);
        }
        for id in [10, 11, 12, 13, 14] {
            assert_eq!(overlay.entity(T, uid(id)), copy.entity(T, uid(id)));
            assert_eq!(Lookup::children(&overlay, uid(id)), copy.children(uid(id)));
            assert_eq!(
                Lookup::descendants(&overlay, uid(id)),
                copy.descendants(uid(id))
            );
        }
        let mut ids: Vec<Uuid> = overlay.entities_of(T).map(Entity::id).collect();
        ids.sort();
        assert_eq!(ids, [10, 11, 12, 13].map(uid));
        // Only the touched entities were copied; the base is unchanged.
        assert_eq!(overlay.copied(), 3);
        assert_eq!(base.children(uid(10)), vec![uid(11)]);
        assert!(!base.knows_op(&uid(5)));
    }

    #[test]
    fn revert_restores_the_base_view() {
        let base = base();
        let mut overlay = Overlay::new(&base);
        overlay.apply(&op(
            5,
            T,
            11,
            "parent_id",
            json!(uid(12).to_string()),
            h(2, 0, "b"),
        ));
        overlay.apply(&op(
            6,
            T,
            13,
            "parent_id",
            json!(uid(10).to_string()),
            h(2, 1, "b"),
        ));
        assert_eq!(Lookup::children(&overlay, uid(10)), vec![uid(13)]);
        assert_eq!(Lookup::children(&overlay, uid(12)), vec![uid(11)]);

        overlay.revert((T, uid(11)));
        overlay.revert((T, uid(13)));
        overlay.revert((T, uid(99)));
        for id in [10, 11, 12, 13] {
            assert_eq!(overlay.entity(T, uid(id)), base.entity(T, uid(id)));
            assert_eq!(Lookup::children(&overlay, uid(id)), base.children(uid(id)));
        }
        assert_eq!(overlay.entities_of(T).count(), 3);
    }
}
