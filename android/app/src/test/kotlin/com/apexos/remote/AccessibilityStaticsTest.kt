package com.apexos.remote

import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import java.io.File

/**
 * The parts of P1-060's accessibility criterion that are decidable by reading.
 *
 * > Accessibility, talkback, large text, rotation, tablets/foldables,
 * > backgrounding, battery saver, and poor-network states are tested.
 *
 * **Most of that is not met and cannot be met from this repository.** There is
 * no device, no emulator and no Robolectric, and `assembleDebug` compiles code
 * it never runs, so nothing here observes TalkBack reading a screen, a rotation
 * surviving, a foldable's hinge, or Doze. Saying so is the point of this file's
 * documentation; what follows is the narrow part that a source scan really can
 * decide, and it is offered as that and not as the criterion.
 *
 * Three properties decide much of a screen's accessibility before any
 * behaviour is observed, and all three are visible in the source:
 *
 * 1. **An icon with no text has no name.** A screen reader announces "button"
 *    and nothing else, and a user who cannot see the glyph has no way to learn
 *    what it does. Every control in this app is a `Button` or `TextButton`
 *    carrying `Text`, which needs no `contentDescription` because the label IS
 *    the name — so the rule enforced here is that no icon-only control appears
 *    at all. That is a stronger and simpler rule than "every Icon has a
 *    description", and it is the one this app already satisfies.
 * 2. **Text sized in anything but `sp` ignores the phone's font scale.** A
 *    user who has set large text gets the same small text, which is the
 *    commonest way a screen fails at large text.
 * 3. **A box that holds text and is sized in `dp` clips it at large text.**
 *    The text obeys the font scale and its container does not, so the label
 *    outgrows the column it sits in. This test found three such columns and
 *    they were fixed rather than exempted; see the fourth case below.
 *
 * ## Why this is a source scan and not an assertion about behaviour
 *
 * The same reasoning as `tools/no-second-write-path.sh`: a rule with no check
 * is a comment. This one cannot prove a screen is usable; it can prove three
 * mistakes that make one unusable are absent, which is worth having on a
 * project where nothing else can look.
 */
class AccessibilityStaticsTest {

    private val sources: List<File> by lazy {
        var dir: File? = File("").absoluteFile
        var root: File? = null
        while (dir != null && root == null) {
            root = listOf(File(dir, "app/src/main/kotlin"), File(dir, "src/main/kotlin"))
                .firstOrNull { it.isDirectory }
            dir = dir.parentFile
        }
        val found = requireNotNull(root) {
            "the app's Kotlin sources were not found from ${File("").absolutePath}. This test " +
                "asserts that something is ABSENT from them, so a run that could not read them " +
                "must fail rather than report that it found nothing."
        }.walkTopDown().filter { it.isFile && it.extension == "kt" }.toList()
        // An empty scan would pass every assertion below while checking
        // nothing, which is the same quiet pass the CI floors exist to stop.
        require(found.size >= 15) { "only ${found.size} Kotlin sources found; the scan is broken" }
        found
    }

    /**
     * A file's lines with the ones that are documentation removed.
     *
     * Written after the first run of this class failed on its own KDoc: the
     * width rule below quotes `Modifier.width(110.dp)` in prose explaining why
     * that shape is wrong, and a scan that reads every line found the
     * explanation and called it the offence. Line-based, so a trailing `//`
     * after real code still counts — that line does contain code.
     */
    private fun code(f: File): List<IndexedValue<String>> =
        f.readLines().withIndex().filterNot { (_, l) ->
            val t = l.trim()
            t.startsWith("*") || t.startsWith("//") || t.startsWith("/*") || t.startsWith("import ")
        }.toList()

    @Test
    fun `no control is an icon with no text beside it`() {
        // `Icon(` in a clickable is the shape that produces an unnamed button.
        // The only `Icon`-family call in this app is a notification's small
        // icon, which is a status-bar drawable and not a control.
        val offenders = sources.filter { f ->
            f.readLines().any { line ->
                val t = line.trim()
                t.startsWith("Icon(") || t.startsWith("IconButton(") || t.startsWith("Icon (")
            }
        }
        assertTrue(
            offenders.isEmpty(),
            "these files draw an icon as a control: ${offenders.map { it.name }}. A screen " +
                "reader announces it as \"button\" and nothing else. Use a TextButton, or give " +
                "the icon a contentDescription and add the file to this test's reasoning.",
        )
    }

    @Test
    fun `text is never sized in dp`() {
        // `fontSize = 14.dp` does not compile against TextUnit, so the real
        // failure is a TextStyle built with a dp-derived value. What is
        // checkable is that no `fontSize` is written with `.dp`.
        for (f in sources) {
            for ((n, line) in code(f)) {
                assertFalse(
                    Regex("""fontSize\s*=\s*[^,)]*\.dp""").containsMatchIn(line),
                    "${f.name}:${n + 1} sizes text in dp, which ignores the phone's font scale",
                )
            }
        }
    }

    @Test
    fun `only the type scale and two derived sizes write an sp value at all`() {
        // A screen that writes its own `14.sp` has stepped outside the type
        // scale, and the scale is what a theme change or a future large-text
        // adjustment moves. Three files are allowed to and each for a stated
        // reason: `Theme.kt` IS the scale; `StateBadge` derives its glyph from
        // the badge's own size, so the two move together; `TerminalView`
        // derives from the terminal text-size setting, which the user sets.
        //
        // The predecessor of this test expected {StateBadge, TerminalView} and
        // was never run. It was wrong in both directions — `TerminalView`
        // writes `textSp.sp`, which its regex (`)\.sp`) could not match, and
        // `Theme.kt` writes two literals it did not know about. A set equality
        // that has never been evaluated is a guess with a test's name on it.
        val writers = sources.filter { f ->
            code(f).any { (_, l) -> Regex("""[0-9A-Za-z_)]\.sp\b""").containsMatchIn(l) }
        }.map { it.name }.toSet()
        assertTrue(
            writers == setOf("Theme.kt", "StateBadge.kt", "TerminalView.kt"),
            "the files writing an sp value are $writers. Each one outside the type scale has to " +
                "earn its place: say what the size is derived from and why that thing follows the " +
                "user's font scale, then add it here.",
        )
    }

    @Test
    fun `no content box pins its width in raw dp`() {
        // A literal `dp` width is a length that does not move when the owner of
        // the phone raises the system font scale. On a box that holds text that
        // is the commonest large-text failure: the label grows, its column does
        // not, and the word wraps or is clipped inside a space sized for the
        // default scale. This round found three of them — the session's gauge
        // labels, its detail fields, and the guide's definition terms, all
        // `Modifier.width(110.dp)` — and they now go through
        // `labelColumnWidth()`, which multiplies by `Density.fontScale`.
        //
        // Below 24.dp a width is a `Spacer` or a hairline rule. Those carry no
        // text, cannot clip any, and are left alone; the threshold is where a
        // box stops being a gap and starts being a container.
        val widths = mutableListOf<String>()
        val pinned = mutableListOf<String>()
        val literal = Regex("""\.(?:required)?(?:width|size)\(\s*([0-9]+(?:\.[0-9]+)?)\.dp""")
        for (f in sources) {
            for ((n, line) in code(f)) {
                for (m in literal.findAll(line)) {
                    widths += "${f.name}:${n + 1}"
                    if (m.groupValues[1].toDouble() >= 24.0) pinned += "${f.name}:${n + 1} ${line.trim()}"
                }
            }
        }
        // Without this the assertion below would also pass on a scan that
        // matched nothing at all — a broken regex reading as a clean codebase.
        assertTrue(
            widths.size >= 10,
            "the width scan matched ${widths.size} lines across ${sources.size} sources, which is " +
                "too few for a Compose app: the regex is broken, not the code.",
        )
        assertTrue(
            pinned.isEmpty(),
            "these pin a content width in raw dp, which ignores the phone's font scale:\n" +
                pinned.joinToString("\n") + "\nUse labelColumnWidth(), or weight(1f), or " +
                "widthIn(min = ...) so the box grows with the text it holds.",
        )
    }
}
