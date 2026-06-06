use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// A Radix Trie for matching hierarchical string topics with wildcards.
///
/// Supports two types of wildcards (matching NATS syntax):
/// - `*` matches exactly one token (e.g., `a.*.c` matches `a.b.c`)
/// - `>` matches one or more trailing tokens (e.g., `a.>` matches `a.b`, `a.b.c`)
#[derive(Debug, Default)]
pub(crate) struct TopicTrie {
    root: TrieNode,
}

#[derive(Debug, Default)]
struct TrieNode {
    /// Subscriptions matching exactly at this node
    subscriptions: HashSet<Uuid>,
    /// Literal children nodes
    children: HashMap<String, TrieNode>,
    /// `*` wildcard child
    any_child: Option<Box<TrieNode>>,
    /// `>` wildcard child
    trailing_child: Option<Box<TrieNode>>,
}

impl TopicTrie {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Insert a subscription into the trie for a given topic pattern
    pub(crate) fn insert(&mut self, topic_pattern: &str, subscription_id: Uuid) {
        let tokens: Vec<&str> = topic_pattern.split('.').collect();
        let mut current = &mut self.root;

        for (i, &token) in tokens.iter().enumerate() {
            if token == ">" {
                if i != tokens.len() - 1 {
                    // Invalid pattern: `>` must be the last token. We'll just ignore trailing stuff.
                }
                if current.trailing_child.is_none() {
                    current.trailing_child = Some(Box::new(TrieNode::default()));
                }
                current = current.trailing_child.as_mut().unwrap();
                break; // `>` must be the end
            } else if token == "*" {
                if current.any_child.is_none() {
                    current.any_child = Some(Box::new(TrieNode::default()));
                }
                current = current.any_child.as_mut().unwrap();
            } else {
                current = current.children.entry(token.to_string()).or_default();
            }
        }

        current.subscriptions.insert(subscription_id);
    }

    /// Remove a subscription from the trie
    /// Note: This does a full scan since we don't know the exact topic pattern it used.
    /// In a real enterprise system, we'd map Uuid -> TopicPattern to optimize removal.
    pub(crate) fn remove(&mut self, subscription_id: Uuid) -> bool {
        Self::remove_recursive(&mut self.root, subscription_id)
    }

    fn remove_recursive(node: &mut TrieNode, subscription_id: Uuid) -> bool {
        let mut found = node.subscriptions.remove(&subscription_id);

        for child in node.children.values_mut() {
            found |= Self::remove_recursive(child, subscription_id);
        }
        if let Some(ref mut child) = node.any_child {
            found |= Self::remove_recursive(child, subscription_id);
        }
        if let Some(ref mut child) = node.trailing_child {
            found |= Self::remove_recursive(child, subscription_id);
        }

        found
    }

    /// Get all subscription IDs that match the given literal topic
    pub(crate) fn match_topic(&self, literal_topic: &str) -> HashSet<Uuid> {
        let tokens: Vec<&str> = literal_topic.split('.').collect();
        let mut results = HashSet::new();
        Self::match_recursive(&self.root, &tokens, 0, &mut results);
        results
    }

    fn match_recursive(
        node: &TrieNode,
        tokens: &[&str],
        depth: usize,
        results: &mut HashSet<Uuid>,
    ) {
        // If we reached the end of the tokens, any subscriptions here match
        if depth == tokens.len() {
            results.extend(&node.subscriptions);
            return;
        }

        let current_token = tokens[depth];

        // 1. Literal match
        if let Some(child) = node.children.get(current_token) {
            Self::match_recursive(child, tokens, depth + 1, results);
        }

        // 2. `*` wildcard match
        if let Some(ref child) = node.any_child {
            Self::match_recursive(child, tokens, depth + 1, results);
        }

        // 3. `>` trailing wildcard match
        if let Some(ref child) = node.trailing_child {
            // `>` matches anything from here to the end
            results.extend(&child.subscriptions);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn test_topic_trie_matching() {
        let mut trie = TopicTrie::new();

        let id_exact = Uuid::new_v4();
        let id_any = Uuid::new_v4();
        let id_trailing = Uuid::new_v4();

        trie.insert("orders.us.created", id_exact);
        trie.insert("orders.*.created", id_any);
        trie.insert("orders.>", id_trailing);

        // Exact match should trigger all 3
        let matches = trie.match_topic("orders.us.created");
        assert!(matches.contains(&id_exact));
        assert!(matches.contains(&id_any));
        assert!(matches.contains(&id_trailing));

        // Different middle token should trigger * and >
        let matches2 = trie.match_topic("orders.eu.created");
        assert!(!matches2.contains(&id_exact));
        assert!(matches2.contains(&id_any));
        assert!(matches2.contains(&id_trailing));

        // Deeper path should only trigger >
        let matches3 = trie.match_topic("orders.us.electronics.created");
        assert!(!matches3.contains(&id_exact));
        assert!(!matches3.contains(&id_any));
        assert!(matches3.contains(&id_trailing));
    }
}
