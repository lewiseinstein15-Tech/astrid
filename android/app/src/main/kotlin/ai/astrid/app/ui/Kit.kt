package ai.astrid.app.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.foundation.text.BasicText
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.platform.testTag

/**
 * Shared kit for every screen (DESIGN.html "Parts"): text helper, chips, cards,
 * list rows, section labels, buttons, form fields, code boxes, the screen
 * header, the five-step bar (Plan · Execute · Check · Verify · Complete),
 * the plan-card step markers, and small stroke icons.
 */
@Composable
fun KitText(
    text: String,
    color: Color,
    size: Int,
    weight: FontWeight,
    family: FontFamily = FontFamily.SansSerif,
    spacing: Int = 0,
    modifier: Modifier = Modifier
) {
    BasicText(
        text = text,
        modifier = modifier,
        style = TextStyle(
            color = color,
            fontSize = size.sp,
            fontWeight = weight,
            fontFamily = family,
            letterSpacing = spacing.sp
        )
    )
}

/** 8dp colored dot (drawer items, step bar, card headers). */
@Composable
fun Dot(color: Color) {
    Box(Modifier.size(8.dp).clip(CircleShape).background(color))
}

/** Chip: 11sp/700 accent text on the accent at 15% alpha, 8dp radius. */
@Composable
fun Chip(label: String, accent: Color) {
    Box(
        Modifier
            .clip(RoundedCornerShape(8.dp))
            .background(tinted(accent))
            .padding(horizontal = 8.dp, vertical = 3.dp)
    ) {
        KitText(label, accent, 11, FontWeight.Bold)
    }
}

/** Standard content card: Surface, 1dp Line border, 14dp radius, 12dp padding. */
@Composable
fun Card(
    verticalArrangement: Arrangement.Vertical = Arrangement.spacedBy(4.dp),
    content: @Composable ColumnScope.() -> Unit
) {
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(14.dp))
            .background(Surface)
            .border(1.dp, Line, RoundedCornerShape(14.dp))
            .padding(12.dp),
        verticalArrangement = verticalArrangement,
        content = content
    )
}

/** 40dp tinted tile with a bold first letter (list rows). */
@Composable
fun LetterTile(letter: String, accent: Color) {
    Box(
        Modifier
            .size(40.dp)
            .clip(RoundedCornerShape(12.dp))
            .background(tinted(accent)),
        contentAlignment = Alignment.Center
    ) {
        KitText(letter, accent, 16, FontWeight.Bold)
    }
}

/** Uppercase 11sp section label in the accent color. */
@Composable
fun SectionLabel(text: String, accent: Color) {
    KitText(text.uppercase(), accent, 11, FontWeight.Bold, spacing = 2)
}

/**
 * List row: tile, title, optional subtitle, optional chip. Tapping fires
 * onClick when given (rows without a click are plain information rows).
 */
@Composable
fun ListRow(
    letter: String,
    accent: Color,
    title: String,
    subtitle: String,
    chipLabel: String? = null,
    chipAccent: Color = accent,
    tag: String? = null,
    onClick: (() -> Unit)? = null
) {
    Row(
        Modifier
            .fillMaxWidth()
            .height(64.dp)
            .then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier)
            .then(if (tag != null) Modifier.testTag(tag) else Modifier)
            .padding(horizontal = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp)
    ) {
        LetterTile(letter, accent)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            KitText(title, TextPrimary, 15, FontWeight.SemiBold)
            if (subtitle.isNotEmpty()) KitText(subtitle, TextMuted, 13, FontWeight.Medium)
        }
        if (chipLabel != null) Chip(chipLabel, chipAccent)
    }
}

/** Filled accent button, 48dp, full width, 14dp radius. */
@Composable
fun FilledButton(label: String, accent: Color, tag: String? = null, onClick: () -> Unit) {
    Box(
        Modifier
            .fillMaxWidth()
            .height(48.dp)
            .clip(RoundedCornerShape(14.dp))
            .background(accent)
            .clickable(onClick = onClick)
            .then(if (tag != null) Modifier.testTag(tag) else Modifier),
        contentAlignment = Alignment.Center
    ) {
        KitText(label, Bg, 15, FontWeight.Bold)
    }
}

/** Outlined button, 48dp, full width. */
@Composable
fun OutlineButton(label: String, accent: Color, tag: String? = null, onClick: () -> Unit) {
    Box(
        Modifier
            .fillMaxWidth()
            .height(48.dp)
            .clip(RoundedCornerShape(14.dp))
            .border(1.dp, accent, RoundedCornerShape(14.dp))
            .clickable(onClick = onClick)
            .then(if (tag != null) Modifier.testTag(tag) else Modifier),
        contentAlignment = Alignment.Center
    ) {
        KitText(label, accent, 15, FontWeight.Bold)
    }
}

/** Single-line text form field with a label above. Tag sits on the field. */
@Composable
fun FormField(label: String, value: String, tag: String? = null, onValue: (String) -> Unit) {
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        KitText(label, TextMuted, 12, FontWeight.SemiBold)
        Box(
            Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(12.dp))
                .background(Surface)
                .border(1.dp, Line, RoundedCornerShape(12.dp))
                .padding(horizontal = 12.dp, vertical = 12.dp)
        ) {
            BasicTextField(
                value = value,
                onValueChange = onValue,
                singleLine = true,
                textStyle = TextStyle(color = TextPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium),
                cursorBrush = SolidColor(AccentOrange),
                modifier = if (tag != null) Modifier.fillMaxWidth().testTag(tag) else Modifier.fillMaxWidth()
            )
            if (value.isEmpty()) {
                KitText("Type here…", TextMuted, 14, FontWeight.Medium)
            }
        }
    }
}

/** Multiline text field (chat composer, dialogs). Tag sits on the field. */
@Composable
fun InputBox(
    value: String,
    hint: String,
    tag: String,
    modifier: Modifier = Modifier,
    onValue: (String) -> Unit
) {
    Box(
        modifier
            .padding(vertical = 2.dp)
    ) {
        BasicTextField(
            value = value,
            onValueChange = onValue,
            textStyle = TextStyle(color = TextPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium),
            cursorBrush = SolidColor(AccentOrange),
            modifier = Modifier.fillMaxWidth().height(40.dp).testTag(tag)
        )
        if (value.isEmpty()) {
            KitText(hint, TextMuted, 15, FontWeight.Medium)
        }
    }
}

/** Code box: mono text on CodeBg, 12dp radius. */
@Composable
fun CodeBox(text: String) {
    Box(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(12.dp))
            .background(CodeBg)
            .border(1.dp, Line, RoundedCornerShape(12.dp))
            .padding(12.dp)
    ) {
        KitText(text, TextMuted, 12, FontWeight.Medium, family = FontFamily.Monospace)
    }
}

/** Screen header: hamburger, star-with-orbit logo, title in the accent. */
@Composable
fun ScreenHeader(
    title: String,
    accent: Color,
    onMenu: () -> Unit,
    trailing: (@Composable RowScope.() -> Unit)? = null
) {
    Row(
        Modifier
            .fillMaxWidth()
            .background(Bg)
            .statusBarsPadding()
            .height(56.dp)
            .padding(horizontal = 16.dp)
            .testTag("screen/$title"),
        verticalAlignment = Alignment.CenterVertically
    ) {
        Box(
            Modifier
                .size(44.dp)
                .clip(RoundedCornerShape(12.dp))
                .clickable(onClick = onMenu)
                .testTag("header/menu"),
            contentAlignment = Alignment.Center
        ) { IconMenu() }
        Spacer(Modifier.width(10.dp))
        KitText(title, accent, 18, FontWeight.Bold)
        Spacer(Modifier.weight(1f))
        trailing?.invoke(this)
    }
}

/** Header + scrollable body + bottom bar padding: the standard screen frame. */
@Composable
fun ScreenScaffold(
    title: String,
    accent: Color,
    onMenu: () -> Unit,
    content: @Composable ColumnScope.() -> Unit
) {
    Column(Modifier.fillMaxSize().background(Bg)) {
        ScreenHeader(title, accent, onMenu)
        Column(
            Modifier
                .weight(1f)
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .navigationBarsPadding()
                .padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp)
        ) { content() }
    }
}

/** Step bar: Plan · Execute · Check · Verify · Complete (DESIGN.html 01-chat). */
@Composable
fun StepBar(current: Int) {
    val steps = listOf("Plan", "Execute", "Check", "Verify", "Complete")
    Row(
        Modifier
            .fillMaxWidth()
            .background(Bg)
            .padding(horizontal = 16.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(5.dp)
    ) {
        steps.forEachIndexed { index, label ->
            Dot(if (index <= current) AccentOrange else Line)
            KitText(
                label,
                if (index <= current) TextPrimary else TextMuted,
                11,
                if (index == current) FontWeight.Bold else FontWeight.Medium
            )
            if (index < steps.lastIndex) Spacer(Modifier.weight(1f))
        }
    }
}

/**
 * Plan-card step marker (01-chat): done = green circle with a check,
 * active = orange ring with an inner dot, pending = gray ring.
 */
@Composable
fun PlanStepMarker(state: PlanStepState) {
    when (state) {
        PlanStepState.DONE -> Box(
            Modifier.size(20.dp).clip(CircleShape).background(AccentGreen),
            contentAlignment = Alignment.Center
        ) { IconCheck(Bg, small = true) }
        PlanStepState.WORKING -> Box(
            Modifier
                .size(20.dp)
                .border(2.dp, AccentOrange, CircleShape),
            contentAlignment = Alignment.Center
        ) { Dot(AccentOrange) }
        PlanStepState.FAILED -> Box(
            Modifier.size(20.dp).clip(CircleShape).background(Danger),
            contentAlignment = Alignment.Center
        ) { IconClose() }
        PlanStepState.PENDING -> Box(Modifier.size(20.dp).border(2.dp, Line, CircleShape))
    }
}

enum class PlanStepState { DONE, WORKING, FAILED, PENDING }

// ---- stroke icons (drawn, no asset dependency) ------------------------------

@Composable
fun IconMenu() {
    Canvas(Modifier.size(20.dp)) {
        val w = 1.8.dp.toPx()
        for (i in 0..2) {
            val y = size.height * (0.25f + 0.25f * i)
            drawLine(TextPrimary, Offset(2f, y), Offset(size.width - 2f, y), w, StrokeCap.Round)
        }
    }
}

@Composable
fun IconClose() {
    Canvas(Modifier.size(18.dp)) {
        val w = 1.8.dp.toPx()
        drawLine(TextMuted, Offset(2f, 2f), Offset(size.width - 2f, size.height - 2f), w, StrokeCap.Round)
        drawLine(TextMuted, Offset(size.width - 2f, 2f), Offset(2f, size.height - 2f), w, StrokeCap.Round)
    }
}

@Composable
fun IconCheck(color: Color, small: Boolean = false) {
    val box = if (small) Modifier.size(12.dp) else Modifier.size(22.dp)
    Canvas(box) {
        val w = if (small) 2.dp.toPx() else 2.dp.toPx()
        drawLine(
            color, Offset(size.width * 0.18f, size.height * 0.55f),
            Offset(size.width * 0.42f, size.height * 0.8f), w, StrokeCap.Round
        )
        drawLine(
            color, Offset(size.width * 0.42f, size.height * 0.8f),
            Offset(size.width * 0.85f, size.height * 0.2f), w, StrokeCap.Round
        )
    }
}

@Composable
fun IconSend(color: Color) {
    // up arrow (DESIGN.html composer send button)
    Canvas(Modifier.size(20.dp)) {
        val w = 2.dp.toPx()
        drawLine(color, Offset(size.width / 2, size.height * 0.85f), Offset(size.width / 2, size.height * 0.18f), w, StrokeCap.Round)
        drawLine(color, Offset(size.width * 0.25f, size.height * 0.42f), Offset(size.width / 2, size.height * 0.18f), w, StrokeCap.Round)
        drawLine(color, Offset(size.width * 0.75f, size.height * 0.42f), Offset(size.width / 2, size.height * 0.18f), w, StrokeCap.Round)
    }
}

@Composable
fun IconChevronDown(color: Color) {
    Canvas(Modifier.size(16.dp)) {
        val w = 2.dp.toPx()
        drawLine(color, Offset(size.width * 0.28f, size.height * 0.38f), Offset(size.width / 2, size.height * 0.62f), w, StrokeCap.Round)
        drawLine(color, Offset(size.width / 2, size.height * 0.62f), Offset(size.width * 0.72f, size.height * 0.38f), w, StrokeCap.Round)
    }
}

@Composable
fun IconChat(color: Color) {
    Canvas(Modifier.size(18.dp)) {
        val w = 1.6.dp.toPx()
        drawRoundRect(
            color, topLeft = Offset(2f, 3f),
            size = Size(size.width - 4f, size.height * 0.6f),
            cornerRadius = CornerRadius(6.dp.toPx(), 6.dp.toPx()), style = Stroke(w)
        )
        drawLine(color, Offset(size.width * 0.3f, size.height * 0.6f), Offset(size.width * 0.3f, size.height * 0.9f), w, StrokeCap.Round)
    }
}

@Composable
fun IconUpload(color: Color) {
    Canvas(Modifier.size(20.dp)) {
        val w = 1.6.dp.toPx()
        drawLine(color, Offset(size.width / 2, size.height * 0.15f), Offset(size.width / 2, size.height * 0.6f), w, StrokeCap.Round)
        drawLine(color, Offset(size.width * 0.28f, size.height * 0.4f), Offset(size.width / 2, size.height * 0.15f), w, StrokeCap.Round)
        drawLine(color, Offset(size.width * 0.72f, size.height * 0.4f), Offset(size.width / 2, size.height * 0.15f), w, StrokeCap.Round)
        drawLine(color, Offset(2f, size.height - 2f), Offset(size.width - 2f, size.height - 2f), w, StrokeCap.Round)
    }
}

@Composable
fun IconCamera(color: Color) {
    Canvas(Modifier.size(20.dp)) {
        val w = 1.6.dp.toPx()
        drawRoundRect(
            color, topLeft = Offset(2f, size.height * 0.3f),
            size = Size(size.width - 4f, size.height * 0.55f),
            cornerRadius = CornerRadius(3.dp.toPx(), 3.dp.toPx()), style = Stroke(w)
        )
        drawCircle(color, center = Offset(size.width / 2, size.height * 0.575f), radius = size.height * 0.14f, style = Stroke(w))
        drawLine(color, Offset(size.width * 0.32f, size.height * 0.3f), Offset(size.width * 0.4f, size.height * 0.18f), w, StrokeCap.Round)
    }
}
