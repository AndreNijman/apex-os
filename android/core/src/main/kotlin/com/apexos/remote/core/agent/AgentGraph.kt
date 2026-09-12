package com.apexos.remote.core.agent

/**
 * The agent graph P1-054's first criterion names, built from what the daemon
 * actually sends.
 *
 * ## It is a forest, and it is intra-session
 *
 * `ChildInfo.parent` names another child **within the same session** — a
 * subagent forked a language server which forked an MCP server is three levels
 * deep. There is no parent-*session* field anywhere in the protocol, and a
 * screen that drew one would be inventing a relationship the runtime does not
 * model. So one session produces one forest, and the sessions themselves are
 * a flat list.
 *
 * ## An orphan is a root, never a disappearance
 *
 * A child whose `parent` names an id that is not in the list becomes a root.
 * That happens for a reason that is not an error: `children` is the daemon's
 * live view, and a parent that exited between two polls is gone from it while
 * its children are not. Dropping the orphans would make a phone show fewer
 * processes than exist, which is the one direction a supervision tool must
 * never be wrong in.
 *
 * A cycle — impossible from the daemon, possible from a corrupted reply — is
 * broken by the same rule, because the walk marks nodes as it places them and
 * never places one twice.
 */
object AgentGraph {
    /** One node, with whatever hangs off it. */
    class Node(val info: ChildInfo, val children: List<Node>) {
        val live: Boolean get() = info.live

        /** This node and everything under it, depth first. */
        fun flatten(): List<Node> = listOf(this) + children.flatMap { it.flatten() }

        override fun toString(): String = "${info.id}(${children.joinToString(",")})"
    }

    /** A node with the depth it is drawn at, for a list that cannot nest. */
    data class Row(val node: Node, val depth: Int) {
        val info: ChildInfo get() = node.info
    }

    /**
     * The forest for one session.
     *
     * Order is preserved from the daemon's list at every level — it reports
     * children in the order they were created, and a tree that re-sorted them
     * would make a process appear to move when a sibling exited.
     */
    fun of(children: List<ChildInfo>): List<Node> {
        if (children.isEmpty()) return emptyList()
        val byId = HashMap<String, ChildInfo>(children.size)
        for (c in children) byId[c.id] = c
        val kids = HashMap<String, MutableList<ChildInfo>>()
        val roots = ArrayList<ChildInfo>()
        for (c in children) {
            val parent = c.parent
            // An orphan is a root: see the note above. `parent == id` is a
            // self-reference, which is a root for the same reason.
            if (parent == null || parent == c.id || parent !in byId) {
                roots.add(c)
            } else {
                kids.getOrPut(parent) { ArrayList() }.add(c)
            }
        }
        val placed = HashSet<String>()
        fun build(info: ChildInfo): Node {
            // Marked before descending, so a cycle cannot make this recurse
            // forever and cannot place a node twice.
            placed.add(info.id)
            val mine = kids[info.id].orEmpty().filter { it.id !in placed }
            return Node(info, mine.map { build(it) })
        }
        val forest = roots.filter { it.id !in placed }.map { build(it) }
        // Anything a cycle stranded is still shown, as a root. Nothing the
        // daemon reported is allowed to vanish.
        val stranded = children.filter { it.id !in placed }.map { Node(it, emptyList()) }
        return forest + stranded
    }

    /** The forest flattened into indented rows, for a list. */
    fun rows(children: List<ChildInfo>): List<Row> {
        val out = ArrayList<Row>()
        fun walk(node: Node, depth: Int) {
            out.add(Row(node, depth))
            for (c in node.children) walk(c, depth + 1)
        }
        for (root in of(children)) walk(root, 0)
        return out
    }

    /** How deep the graph goes. Zero for a session with no children. */
    fun depth(children: List<ChildInfo>): Int =
        rows(children).maxOfOrNull { it.depth + 1 } ?: 0

    /**
     * A one-line summary for a row that has no room for a tree.
     *
     * Subagents and processes counted apart, because they are different news:
     * three subagents is a session doing parallel work, and three processes is
     * a session that started a language server. Only live ones are counted —
     * a list that included the dead would grow all afternoon and never shrink.
     */
    fun summary(children: List<ChildInfo>): String? {
        val live = children.filter { it.live }
        if (live.isEmpty()) return null
        val subagents = live.count { it.isSubagent }
        val processes = live.size - subagents
        val parts = ArrayList<String>(2)
        if (subagents > 0) parts.add("$subagents subagent" + if (subagents == 1) "" else "s")
        if (processes > 0) parts.add("$processes process" + if (processes == 1) "" else "es")
        return parts.joinToString(", ")
    }
}
