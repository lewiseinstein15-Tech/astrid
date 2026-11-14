package ai.astrid.app.ui

import androidx.compose.ui.graphics.Color

/**
 * Design tokens (DESIGN.html): charcoal surfaces, one hue per section, orange
 * as the brand accent. Every screen draws only with these. The user bubble
 * and borders come straight from the 01-chat mockup.
 */
val Bg = Color(0xFF1C1917)
val Surface = Color(0xFF292524)
val BubbleUser = Color(0xFF3A332F)
val CodeBg = Color(0xFF141419)
val Line = Color(0xFF3A332F)
val TextPrimary = Color(0xFFF5EFEA)
val TextMuted = Color(0xFFA8A29E)
val Danger = Color(0xFFE5484D)

val AccentOrange = Color(0xFFFB923C)
val AccentAmber = Color(0xFFD97706)
val AccentCoral = Color(0xFFF97066)
val AccentBlue = Color(0xFF60A5FA)
val AccentPurple = Color(0xFFA78BFA)
val AccentGreen = Color(0xFF4ADE80)
val AccentTeal = Color(0xFF2DD4BF)
val AccentPink = Color(0xFFF472B6)
val AccentYellow = Color(0xFFFACC15)
val AccentIndigo = Color(0xFF818CF8)
val AccentSky = Color(0xFF38BDF8)
val AccentSlate = Color(0xFF94A3B8)

/** Accent at 15% alpha — chip and tile fills (DESIGN.html "FB923C26"). */
fun tinted(c: Color) = c.copy(alpha = 0.15f)
