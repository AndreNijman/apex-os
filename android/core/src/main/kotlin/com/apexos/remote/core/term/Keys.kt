package com.apexos.remote.core.term

import kotlinx.serialization.Serializable

/**
 * The keys a phone has to be able to send even though it has no keyboard.
 *
 * Named rather than numbered: Android's `KeyEvent` codes are Android's, and
 * `:core` is a plain JVM module on purpose. The app maps one to the other in
 * one place.
 */
enum class Key {
    ENTER, TAB, BACKSPACE, ESCAPE, DELETE, INSERT,
    UP, DOWN, LEFT, RIGHT,
    HOME, END, PAGE_UP, PAGE_DOWN,
    F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
}

/**
 * Which of the three modifiers were held.
 *
 * `meta` is deliberately absent. A terminal has no encoding for it, so an app
 * that carried one would be carrying a state it could never send.
 */
data class Mods(val ctrl: Boolean = false, val alt: Boolean = false, val shift: Boolean = false) {
    val none: Boolean get() = !ctrl && !alt && !shift

    /**
     * xterm's modifier parameter: `1 + shift + 2*alt + 4*ctrl`.
     *
     * One-based because zero would be an absent parameter, and `ESC[1;1A` —
     * "up with no modifiers" — is a thing a terminal must be able to say.
     */
    val param: Int get() = 1 + (if (shift) 1 else 0) + (if (alt) 2 else 0) + (if (ctrl) 4 else 0)

    companion object {
        val NONE = Mods()
    }
}

/**
 * Turning a key press into the bytes a program on the far end will recognise.
 *
 * ## Why this is in `:core` and not in the Compose layer
 *
 * Because it is the half of the terminal that can be wrong silently. A screen
 * that renders the wrong colour is visibly wrong; an arrow key encoded as
 * `ESC[A` to a program that asked for `ESC OA` simply does nothing, and the
 * user concludes the connection is bad. Every rule below is one a test names.
 */
object Keys {
    private const val ESC = '\u001b'
    private const val CSI = "\u001b["
    private const val SS3 = "\u001bO"

    /**
     * The bytes for one key press.
     *
     * [applicationCursor] is [Terminal.applicationCursorKeys] — DECCKM, which
     * the far end turns on and which changes the arrows and Home/End from CSI
     * to SS3. It is a parameter rather than read from a terminal here so that
     * the encoding stays a pure function: the mistake this prevents is a
     * keyboard that consults a terminal it is no longer attached to.
     */
    fun encode(key: Key, mods: Mods = Mods.NONE, applicationCursor: Boolean = false): ByteArray =
        encodeToString(key, mods, applicationCursor).toByteArray(Charsets.UTF_8)

    internal fun encodeToString(key: Key, mods: Mods, applicationCursor: Boolean): String {
        // A modified key always takes the CSI form with a modifier parameter,
        // even in application-cursor mode: `ESC O 1 ; 5 A` is not a sequence
        // any terminal emits, and xterm switches to `ESC [ 1 ; 5 A` instead.
        val modified = !mods.none
        return when (key) {
            Key.ENTER -> prefixAlt("\r", mods)
            Key.TAB -> if (mods.shift) "${CSI}Z" else prefixAlt("\t", mods)
            // DEL and not BS. Every terminal emulator written this century
            // sends 0x7F for the backspace key and reserves 0x08 for Ctrl-H,
            // and a readline that receives 0x08 deletes nothing.
            Key.BACKSPACE -> if (mods.ctrl) "\u0008" else prefixAlt("\u007f", mods.copy(ctrl = false))
            Key.ESCAPE -> prefixAlt("$ESC", mods)
            Key.UP -> cursor('A', mods, applicationCursor, modified)
            Key.DOWN -> cursor('B', mods, applicationCursor, modified)
            Key.RIGHT -> cursor('C', mods, applicationCursor, modified)
            Key.LEFT -> cursor('D', mods, applicationCursor, modified)
            Key.HOME -> cursor('H', mods, applicationCursor, modified)
            Key.END -> cursor('F', mods, applicationCursor, modified)
            Key.INSERT -> tilde(2, mods)
            Key.DELETE -> tilde(3, mods)
            Key.PAGE_UP -> tilde(5, mods)
            Key.PAGE_DOWN -> tilde(6, mods)
            // F1..F4 are SS3 letters, not tildes, and they are SS3 whatever
            // DECCKM says: they predate it.
            Key.F1 -> functionLetter('P', mods)
            Key.F2 -> functionLetter('Q', mods)
            Key.F3 -> functionLetter('R', mods)
            Key.F4 -> functionLetter('S', mods)
            Key.F5 -> tilde(15, mods)
            Key.F6 -> tilde(17, mods)
            Key.F7 -> tilde(18, mods)
            Key.F8 -> tilde(19, mods)
            Key.F9 -> tilde(20, mods)
            Key.F10 -> tilde(21, mods)
            Key.F11 -> tilde(23, mods)
            Key.F12 -> tilde(24, mods)
        }
    }

    private fun cursor(letter: Char, mods: Mods, applicationCursor: Boolean, modified: Boolean): String = when {
        modified -> "${CSI}1;${mods.param}$letter"
        applicationCursor -> "$SS3$letter"
        else -> "$CSI$letter"
    }

    private fun tilde(number: Int, mods: Mods): String =
        if (mods.none) "$CSI$number~" else "$CSI$number;${mods.param}~"

    private fun functionLetter(letter: Char, mods: Mods): String =
        if (mods.none) "$SS3$letter" else "${CSI}1;${mods.param}$letter"

    private fun prefixAlt(body: String, mods: Mods): String =
        if (mods.alt) "$ESC$body" else body

    /**
     * Ordinary typed text.
     *
     * Ctrl folds the character into a control code by the ASCII rule — clear
     * bit 6 — and that rule is why Ctrl-C is 0x03 and Ctrl-[ is escape. Alt
     * prefixes an escape, which is how every terminal has sent Meta since
     * before it was called Alt.
     */
    fun text(text: String, mods: Mods = Mods.NONE): ByteArray {
        if (text.isEmpty()) return ByteArray(0)
        if (mods.ctrl) {
            val sb = StringBuilder()
            for (ch in text) sb.append(controlChar(ch))
            return prefixAlt(sb.toString(), mods).toByteArray(Charsets.UTF_8)
        }
        return prefixAlt(text, mods).toByteArray(Charsets.UTF_8)
    }

    /**
     * One character with Ctrl held.
     *
     * The cases outside `@`..`_` are the ones people actually press and the
     * ones a naive `and 0x1f` gets wrong: Ctrl-Space is NUL, Ctrl-? is DEL
     * (0x7F, not 0x3F and 0x1F), and a letter must be upper-cased first or
     * Ctrl-a becomes 0x61 and 0x1f disagree.
     */
    fun controlChar(ch: Char): Char = when (ch) {
        ' ', '@' -> '\u0000'
        '?' -> '\u007f'
        in 'a'..'z' -> (ch.code - 'a'.code + 1).toChar()
        in 'A'..'Z' -> (ch.code - 'A'.code + 1).toChar()
        '[' -> '\u001b'
        '\\' -> '\u001c'
        ']' -> '\u001d'
        '^' -> '\u001e'
        '_' -> '\u001f'
        // Not a character Ctrl does anything to. Sent as itself rather than
        // mangled: a terminal that turned Ctrl-1 into 0x11 would be sending
        // Ctrl-Q, which stops the terminal.
        else -> ch
    }

    /**
     * A paste, in the form the far end asked for.
     *
     * Two things happen here and both are safety rather than fidelity:
     *
     * * When bracketed paste is on, the text is wrapped in `ESC[200~` and
     *   `ESC[201~` so the program knows the bytes were pasted and not typed —
     *   which is what stops a shell running a pasted newline.
     * * The end marker is stripped out of the text first. Without that, a
     *   paste containing `ESC[201~` would end its own bracket, and everything
     *   after it would arrive as if typed: a paste that runs commands, from a
     *   clipboard the phone's owner did not necessarily fill.
     *
     * Newlines become carriage returns because that is what a terminal
     * delivers when the return key is pressed, and a program reading a line
     * is waiting for CR.
     */
    fun paste(text: String, bracketed: Boolean): ByteArray {
        val body = text
            .replace(END_MARKER, "")
            .replace("\r\n", "\r")
            .replace('\n', '\r')
        return if (bracketed) {
            (START_MARKER + body + END_MARKER).toByteArray(Charsets.UTF_8)
        } else {
            body.toByteArray(Charsets.UTF_8)
        }
    }

    const val START_MARKER: String = "\u001b[200~"
    const val END_MARKER: String = "\u001b[201~"

    /**
     * The bytes an accessory-row button sends, or `null` when it is a
     * modifier that arms the next press instead.
     */
    fun accessory(action: AccessoryAction, applicationCursor: Boolean, armed: Mods): ByteArray? = when (action) {
        is AccessoryAction.Modifier -> null
        is AccessoryAction.Press -> encode(action.key, armed, applicationCursor)
        is AccessoryAction.Text -> text(action.text, armed)
    }
}

/** What an accessory button does when it is pressed. */
sealed class AccessoryAction {
    /** Arms a modifier for the next key, the way a phone's shift key works. */
    data class Modifier(val ctrl: Boolean = false, val alt: Boolean = false) : AccessoryAction()

    data class Press(val key: Key) : AccessoryAction()

    data class Text(val text: String) : AccessoryAction()
}

/**
 * One button on the row above the keyboard.
 *
 * Stored rather than hard-coded because the criterion says *configurable*,
 * and because the right row genuinely differs per person: somebody who lives
 * in `vim` wants `Esc` and `:`; somebody driving Claude Code wants `Tab`,
 * `Esc` and the arrows. It is a `MachineStore` setting, which is what keeps
 * `:app` from opening a second file — see `tools/no-second-write-path.sh`.
 */
@Serializable
data class AccessoryKey(
    /** What is printed on the button. */
    val label: String,
    /**
     * What it does, as text.
     *
     * A string and not a sealed class, because this is persisted JSON and a
     * sealed hierarchy in a stored document is a migration every time a
     * variant is added. [AccessoryKeys.parse] turns it into one.
     */
    val action: String,
)

/** The accessory row: parsing, defaults, and the vocabulary of [AccessoryKey.action]. */
object AccessoryKeys {
    /**
     * `ctrl`, `alt`, `key:<NAME>` or `text:<literal>`.
     *
     * Returns `null` for an action this build does not know rather than
     * throwing. A stored row written by a newer version must not stop the app
     * opening; the button it cannot render is simply not shown.
     */
    fun parse(action: String): AccessoryAction? = when {
        action == "ctrl" -> AccessoryAction.Modifier(ctrl = true)
        action == "alt" -> AccessoryAction.Modifier(alt = true)
        action.startsWith("key:") ->
            runCatching { Key.valueOf(action.removePrefix("key:")) }.getOrNull()?.let { AccessoryAction.Press(it) }
        action.startsWith("text:") -> AccessoryAction.Text(action.removePrefix("text:"))
        else -> null
    }

    /**
     * The row that ships.
     *
     * Chosen from what the four agent TUIs actually bind: `Esc` cancels in all
     * of them, `Tab` completes, `Ctrl` reaches `Ctrl-C` and `Ctrl-R`, the
     * arrows move through history and through a list, and `/` opens the
     * command menu in Claude Code and in OpenCode both.
     */
    val DEFAULT: List<AccessoryKey> = listOf(
        AccessoryKey("Esc", "key:ESCAPE"),
        AccessoryKey("Ctrl", "ctrl"),
        AccessoryKey("Alt", "alt"),
        AccessoryKey("Tab", "key:TAB"),
        AccessoryKey("/", "text:/"),
        AccessoryKey("|", "text:|"),
        AccessoryKey("←", "key:LEFT"),
        AccessoryKey("↓", "key:DOWN"),
        AccessoryKey("↑", "key:UP"),
        AccessoryKey("→", "key:RIGHT"),
        AccessoryKey("Home", "key:HOME"),
        AccessoryKey("End", "key:END"),
        AccessoryKey("PgUp", "key:PAGE_UP"),
        AccessoryKey("PgDn", "key:PAGE_DOWN"),
    )
}
