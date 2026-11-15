package ai.astrid.app.ui

import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import ai.astrid.app.R
import ai.astrid.app.TurnService
import java.io.File
import java.security.MessageDigest
import kotlinx.coroutines.delay
import uniffi.astrid_ffi.Bridge
import uniffi.astrid_ffi.ChatMessageRecord
import uniffi.astrid_ffi.EventRecord

/**
 * Screen 01 "Chat" — REAL, and shaped exactly like the 01-chat mockup:
 * step bar Plan·Execute·Check·Verify·Complete; the Plan card and agent
 * updates rendered from the core's REAL receipt stream (no ids, no demo
 * rows); the composer as one card — input on top, below it the + button,
 * the model chip (kilo-auto/free) and the send button on the right, with
 * imePadding so the keyboard never covers it. Attachments picked via SAF or
 * captured by the camera become chips above the input; Send sends the
 * message with the files; the bubble shows them; the same sha256 is one chip.
 */

/** One attachment waiting above the input. */
data class AttachedFile(
    val name: String,
    val bytes: Long,
    val sha256: String,
    val path: String,
    val isPhoto: Boolean,
)

/** Copy a picked uri into app cache and describe it for the chip row. */
fun attachFromUri(context: android.content.Context, uri: Uri): AttachedFile? = runCatching {
    val name = (uri.lastPathSegment ?: "file.bin").substringAfterLast('/').ifEmpty { "file.bin" }
    // one directory per pick: the file keeps its original name (it becomes
    // the upload name and the bubble chip), isolation comes from the dir
    val dir = File(File(context.cacheDir, "picks"), System.currentTimeMillis().toString()).apply { mkdirs() }
    val dest = File(dir, name)
    context.contentResolver.openInputStream(uri)?.use { input ->
        dest.outputStream().use { output -> input.copyTo(output) }
    } ?: return null
    val sha = MessageDigest.getInstance("SHA-256")
        .digest(dest.readBytes())
        .joinToString("") { "%02x".format(it) }
    AttachedFile(name, dest.length(), sha, dest.absolutePath, isPhoto = false)
}.getOrNull()

/** Describe an already-on-disk capture (photo) for the chip row. */
fun attachFromFile(file: File): AttachedFile? = runCatching {
    if (!file.exists()) return null
    val sha = MessageDigest.getInstance("SHA-256")
        .digest(file.readBytes())
        .joinToString("") { "%02x".format(it) }
    AttachedFile(file.name, file.length(), sha, file.absolutePath, isPhoto = true)
}.getOrNull()

/** The same file (same sha256) shows once, never twice. */
fun attachAdd(list: List<AttachedFile>, file: AttachedFile): List<AttachedFile> =
    if (list.any { it.sha256 == file.sha256 }) list else list + file

fun bytesLabel(bytes: Long): String = when {
    bytes >= 1_048_576 -> "%.1f MB".format(bytes / 1_048_576.0)
    bytes >= 1024 -> "%.1f KB".format(bytes / 1024.0)
    else -> "$bytes B"
}

@Composable
fun ChatScreen(
    bridge: Bridge,
    chatEpoch: Int = 0,
    onMenu: () -> Unit = {},
    onModelClick: () -> Unit = {},
    onNewChat: () -> Unit = {}
) {
    val context = LocalContext.current
    val info = remember { bridge.appInfo() }
    var messages by remember { mutableStateOf(loadMessages(bridge)) }
    var sending by remember { mutableStateOf(false) }
    var pendingSend by remember { mutableStateOf<String?>(null) }
    var attachments by remember { mutableStateOf(listOf<AttachedFile>()) }
    var attachOpen by remember { mutableStateOf(false) }
    var events by remember { mutableStateOf(listOf<EventRecord>()) }
    var pendingCameraFile by remember { mutableStateOf<File?>(null) }
    var composerText by remember { mutableStateOf("") }
    // Phase 4: honest fallback receipt — why a non-first ladder rung served
    // the last turn (null when the first rung served).
    var fallbackReason by remember { mutableStateOf<String?>(null) }
    // Phase 4: long turns promote to a foreground service. The rationale is
    // asked once per session; a decline means never promote this session.
    var rationaleAsked by remember { mutableStateOf(false) }
    var rationaleAccepted by remember { mutableStateOf(false) }
    var servicePromoted by remember { mutableStateOf(false) }
    var showTurnRationale by remember { mutableStateOf(false) }

    // Reload whenever a new chat starts or a session is opened.
    LaunchedEffect(chatEpoch) {
        messages = loadMessages(bridge)
    }
    // The send runs the REAL core turn with the attachments; every state
    // write stays inside composition so the transcript always reflects the
    // Rust store.
    LaunchedEffect(pendingSend) {
        val text = pendingSend ?: return@LaunchedEffect
        sending = true
        val paths = attachments.map { it.path }
        runCatching { bridge.sendMessageWithFiles(text, paths) }
            .onSuccess { reply ->
                // Phase 4: keep the honest fallback receipt from the reply.
                fallbackReason = reply.fallbackReason
            }
            .onFailure { e ->
                runCatching { bridge.appendEvent("chat.error", "${e::class.simpleName}: ${e.message}") }
            }
        attachments = emptyList()
        messages = loadMessages(bridge)
        sending = false
        pendingSend = null
    }
    // Receipt stream for the Plan card and agent updates (real audit log).
    LaunchedEffect(bridge) {
        while (true) {
            delay(2000)
            events = runCatching { bridge.recentEvents(60u) }.getOrDefault(events)
        }
    }

    // Phase 4: a turn still working after the promote delay moves to the
    // foreground service so Android cannot kill it mid-task. Short turns —
    // and every test turn — finish before the delay fires and stay background.
    LaunchedEffect(sending) {
        if (!sending) {
            if (servicePromoted) {
                TurnService.stop(context)
                servicePromoted = false
            }
            return@LaunchedEffect
        }
        delay(TurnService.PROMOTE_DELAY_MS)
        if (!sending) return@LaunchedEffect
        when {
            !rationaleAsked -> showTurnRationale = true
            rationaleAccepted -> {
                TurnService.start(context)
                servicePromoted = true
            }
        }
    }

    // SAF pick: opens at once from the attach menu; result becomes a chip.
    val filePicker = rememberLauncherForActivityResult(ActivityResultContracts.GetContent()) { uri ->
        if (uri != null) {
            attachFromUri(context, uri)?.let { picked ->
                attachments = attachAdd(attachments, picked)
                runCatching { bridge.appendEvent("attach.picked", picked.name) }
            }
        }
        attachOpen = false
    }
    // Camera capture: written app-privately via FileProvider, then a chip.
    val camera = rememberLauncherForActivityResult(ActivityResultContracts.TakePicture()) { ok ->
        val file = pendingCameraFile
        if (ok && file != null && file.exists()) {
            attachFromFile(file)?.let { picked ->
                attachments = attachAdd(attachments, picked)
                runCatching { bridge.recordPhoto(file.absolutePath) }
                runCatching { bridge.appendEvent("attach.photo", picked.name) }
            }
        } else if (!ok) {
            file?.delete()
        }
        pendingCameraFile = null
        attachOpen = false
    }

    val listState = rememberLazyListState()
    val imeBottom = WindowInsets.ime.getBottom(LocalDensity.current)
    LaunchedEffect(messages.size, imeBottom) {
        if (messages.isNotEmpty()) listState.animateScrollToItem(messages.lastIndex)
    }

    fun takePhoto() {
        runCatching {
            val dir = File(context.filesDir, "photos").apply { mkdirs() }
            val file = File(dir, "photo_${System.currentTimeMillis()}.jpg")
            pendingCameraFile = file
            val uri = androidx.core.content.FileProvider.getUriForFile(
                context, "ai.astrid.app.fileprovider", file
            )
            camera.launch(uri)
        }
    }

    Column(
        Modifier
            .fillMaxSize()
            .background(Bg)
            .testTag("screen/Chat")
    ) {
        Header(onMenu)
        StepBar(current = 1)

        LazyColumn(
            Modifier
                .weight(1f)
                .fillMaxWidth()
                .padding(horizontal = 16.dp),
            state = listState,
            verticalArrangement = Arrangement.spacedBy(10.dp)
        ) {
            item { CoreCard(info, bridge.greeting()) }
            item { PlanCard(goal = goalOf(messages), events = events, sending = sending) }
            if (messages.isEmpty()) {
                item {
                    Card {
                        KitText(
                            "Say hello, or hand me real work. Everything runs in the Rust core on this device.",
                            TextMuted, 14, FontWeight.Medium
                        )
                    }
                }
            }
            items(messages, key = { it.id }) { msg ->
                MessageRow(msg)
            }
            if (sending) {
                item {
                    Row(
                        Modifier.testTag("chat/thinking"),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(8.dp)
                    ) {
                        Dot(AccentOrange)
                        KitText("Astrid is thinking…", TextMuted, 13, FontWeight.Medium)
                    }
                }
            }
            item { Spacer(Modifier.height(4.dp)) }
        }

        // Phase 4: honest fallback receipt — why a non-first ladder rung
        // served the last turn. Shown under the transcript, muted, verbatim.
        fallbackReason?.let { reason ->
            Row(
                Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 16.dp)
                    .testTag("receipt/fallback")
            ) {
                KitText("fallback: $reason", TextMuted, 11, FontWeight.Medium)
            }
        }

        // Composer card (01-chat): input on top; +, model chip, send right.
        Column(
            Modifier
                .fillMaxWidth()
                .navigationBarsPadding()
                .imePadding()
                .padding(horizontal = 16.dp, vertical = 8.dp)
        ) {
            Box(
                Modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(22.dp))
                    .background(Surface)
                    .border(1.dp, Line, RoundedCornerShape(22.dp))
                    .padding(horizontal = 12.dp, vertical = 10.dp)
            ) {
                Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    if (attachments.isNotEmpty()) {
                        Row(
                            Modifier
                                .fillMaxWidth()
                                .padding(bottom = 4.dp),
                            horizontalArrangement = Arrangement.spacedBy(8.dp)
                        ) {
                            attachments.forEachIndexed { index, file ->
                                AttachmentChip(
                                    file = file,
                                    tag = "attach/chip/$index",
                                    onRemove = {
                                        attachments = attachments.filterIndexed { i, _ -> i != index }
                                    }
                                )
                            }
                        }
                    }
                    InputBox(
                        value = composerText,
                        hint = "Ask Astrid",
                        tag = "chat/input",
                        modifier = Modifier.fillMaxWidth()
                    ) { composerText = it }
                    Row(
                        Modifier.fillMaxWidth(),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(8.dp)
                    ) {
                        Box(
                            Modifier
                                .size(44.dp)
                                .clip(CircleShape)
                                .border(1.dp, Line, CircleShape)
                                .clickable { attachOpen = true }
                                .testTag("composer/attach"),
                            contentAlignment = Alignment.Center
                        ) {
                            KitText("+", TextPrimary, 22, FontWeight.Bold)
                        }
                        Box(
                            Modifier
                                .height(44.dp)
                                .clip(RoundedCornerShape(22.dp))
                                .background(tinted(AccentOrange))
                                .clickable(onClick = onModelClick)
                                .testTag("composer/model")
                                .padding(horizontal = 14.dp),
                            contentAlignment = Alignment.Center
                        ) {
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                KitText(providerChipLabel(bridge), AccentOrange, 14, FontWeight.SemiBold)
                                Spacer(Modifier.width(6.dp))
                                IconChevronDown(AccentOrange)
                            }
                        }
                        Spacer(Modifier.weight(1f))
                        Box(
                            Modifier
                                .size(44.dp)
                                .clip(CircleShape)
                                .background(if (composerText.isNotBlank() && !sending) AccentOrange else Line)
                                .clickable(enabled = composerText.isNotBlank() && !sending) {
                                    val text = composerText.trim()
                                    composerText = ""
                                    pendingSend = text
                                }
                                .testTag("chat/send"),
                            contentAlignment = Alignment.Center
                        ) {
                            IconSend(if (composerText.isNotBlank() && !sending) Bg else TextMuted)
                        }
                    }
                }
            }
        }
    }

    if (attachOpen) {
        AttachMenu(
            onNewChat = {
                attachOpen = false
                onNewChat()
            },
            onUploadFile = { filePicker.launch("*/*") },
            onTakePhoto = { takePhoto() },
            onClose = { attachOpen = false }
        )
    }

    if (showTurnRationale) {
        TurnRationaleDialog(
            onAccept = {
                showTurnRationale = false
                rationaleAsked = true
                rationaleAccepted = true
                // The turn that triggered the dialog is usually still running;
                // promote now. Later turns promote without asking again.
                if (sending) {
                    TurnService.start(context)
                    servicePromoted = true
                }
            },
            onDecline = {
                showTurnRationale = false
                rationaleAsked = true
                rationaleAccepted = false
            }
        )
    }
}

private fun loadMessages(bridge: Bridge): List<ChatMessageRecord> =
    runCatching { bridge.recentMessages(200u) }.getOrDefault(emptyList())

private fun goalOf(messages: List<ChatMessageRecord>): String =
    messages.lastOrNull { it.role == "user" }?.text?.take(60) ?: ""

/**
 * The chip label is the model the ladder will actually serve:
 * the configured model, else the provider name, else the built-in
 * kilo-auto/free automatic rung.
 */
fun providerChipLabel(bridge: Bridge): String {
    val provider = runCatching { bridge.provider() }.getOrNull()
    return when {
        provider != null && provider.baseUrl.isNotEmpty() && provider.model.isNotEmpty() -> provider.model
        provider != null && provider.baseUrl.isNotEmpty() -> provider.name
        else -> "kilo-auto/free"
    }
}

/**
 * The Plan card and agent updates — REAL rows from the core's receipt
 * stream: narration lines, tool receipts and their states, sliced to the
 * latest turn. No raw ids are ever rendered.
 */
@Composable
fun PlanCard(goal: String, events: List<EventRecord>, sending: Boolean) {
    // events are newest-first; the latest turn is everything up to (but not
    // including) the next turn.started row.
    val turnRows = events.takeWhile { it.kind != "turn.started" }
        .filter { it.kind != "turn.completed" }
        .filter { !it.text.startsWith("turn_") }
        .asReversed() // oldest first
    if (turnRows.isEmpty()) return

    val steps = turnRows.map { row ->
        val state = when {
            row.kind == "tool.completed" || row.kind == "narration.completion" -> PlanStepState.DONE
            row.kind == "tool.refused" || row.kind == "tool.failed" -> PlanStepState.FAILED
            row.kind == "narration.ack" -> PlanStepState.PENDING
            else -> PlanStepState.WORKING
        }
        val label = when {
            row.kind.startsWith("tool.") -> row.text
            else -> "${row.kind.removePrefix("narration.")}: ${row.text}"
        }
        state to label
    }
    val done = steps.count { it.first == PlanStepState.DONE }

    Card(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            KitText("Plan", AccentOrange, 16, FontWeight.Bold)
            Spacer(Modifier.weight(1f))
            if (sending) Chip("Live", AccentGreen)
            Spacer(Modifier.width(8.dp))
            KitText("${done} of ${steps.size} done", TextMuted, 12, FontWeight.Medium)
        }
        if (goal.isNotEmpty()) {
            KitText("Goal: $goal", TextMuted, 12, FontWeight.Medium)
        }
        Box(
            Modifier
                .fillMaxWidth()
                .height(6.dp)
                .clip(RoundedCornerShape(3.dp))
                .background(Line)
        ) {
            val fraction = if (steps.isEmpty()) 0f else done.toFloat() / steps.size
            Box(
                Modifier
                    .fillMaxWidth(fraction)
                    .height(6.dp)
                    .clip(RoundedCornerShape(3.dp))
                    .background(AccentOrange)
            )
        }
        steps.forEachIndexed { index, (state, label) ->
            Row(
                Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(10.dp)
            ) {
                PlanStepMarker(state)
                KitText(label.take(120), TextMuted, 14, FontWeight.Medium, modifier = Modifier.weight(1f))
                if (state == PlanStepState.WORKING && index == steps.lastIndex) Chip("Working", AccentOrange)
            }
        }
    }
}

/** The core card: every string comes from Rust through the bridge. */
@Composable
private fun CoreCard(info: uniffi.astrid_ffi.AppInfo, greeting: String) {
    Card {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Image(
                painter = painterResource(R.drawable.logo_mark),
                contentDescription = null,
                modifier = Modifier.size(20.dp)
            )
            Spacer(Modifier.width(8.dp))
            KitText("${info.name} core", TextPrimary, 14, FontWeight.Bold)
            Spacer(Modifier.weight(1f))
            Chip("v" + info.version, AccentOrange)
        }
        KitText(greeting, TextMuted, 12, FontWeight.Medium)
    }
}

/** One transcript row — user bubbles right-aligned (design), files as chips. */
@Composable
fun MessageRow(msg: ChatMessageRecord) {
    val isUser = msg.role == "user"
    Column(
        Modifier
            .fillMaxWidth()
            .testTag("msg/${msg.role}")
    ) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp)
        ) {
            if (!isUser) {
                Dot(AccentOrange)
                KitText("Astrid", TextMuted, 11, FontWeight.Bold)
                if (msg.provider.isNotEmpty()) Chip(msg.provider, AccentGreen)
            } else {
                Spacer(Modifier.weight(1f))
                KitText("You", TextMuted, 11, FontWeight.Bold)
                Dot(AccentSlate)
            }
        }
        Spacer(Modifier.height(4.dp))
        Box(
            Modifier
                .fillMaxWidth()
                .clip(
                    RoundedCornerShape(
                        topStart = 16.dp, topEnd = 16.dp,
                        bottomEnd = if (isUser) 4.dp else 16.dp,
                        bottomStart = if (isUser) 16.dp else 4.dp
                    )
                )
                .background(if (isUser) BubbleUser else Surface)
                .border(1.dp, Line, RoundedCornerShape(16.dp))
                .padding(12.dp)
        ) {
            Column {
                KitText(msg.text, TextPrimary, 14, FontWeight.Medium)
                if (msg.files.isNotEmpty()) {
                    Spacer(Modifier.height(6.dp))
                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        msg.files.forEach { name ->
                            Chip(name, AccentOrange)
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun AttachmentChip(file: AttachedFile, tag: String, onRemove: () -> Unit) {
    Row(
        Modifier
            .clip(RoundedCornerShape(12.dp))
            .background(tinted(AccentBlue))
            .border(1.dp, AccentBlue.copy(alpha = 0.4f), RoundedCornerShape(12.dp))
            .padding(start = 10.dp, end = 4.dp, top = 4.dp, bottom = 4.dp)
            .testTag(tag),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp)
    ) {
        Column {
            KitText(file.name.take(24), TextPrimary, 12, FontWeight.SemiBold)
            KitText(bytesLabel(file.bytes), TextMuted, 10, FontWeight.Medium)
        }
        Box(
            Modifier
                .size(24.dp)
                .clip(CircleShape)
                .clickable(onClick = onRemove)
                .testTag(tag.replace("chip", "remove")),
            contentAlignment = Alignment.Center
        ) {
            KitText("×", TextMuted, 16, FontWeight.Bold)
        }
    }
}

/** Attach menu (03-attach-menu): New chat / Upload file / Take photo. */
@Composable
private fun AttachMenu(
    onNewChat: () -> Unit,
    onUploadFile: () -> Unit,
    onTakePhoto: () -> Unit,
    onClose: () -> Unit
) {
    Box(Modifier.fillMaxSize().clickable(onClick = onClose).background(Bg.copy(alpha = 0.55f))) {
        Column(
            Modifier
                .align(Alignment.BottomStart)
                .padding(start = 16.dp, bottom = 120.dp)
                .width(230.dp)
                .clip(RoundedCornerShape(16.dp))
                .background(Surface)
                .border(1.dp, Line, RoundedCornerShape(16.dp))
                .padding(6.dp)
        ) {
            MenuItem("New chat", "new-chat", AccentOrange, { IconChat(AccentOrange) }, onNewChat)
            MenuItem("Upload file", "upload-file", AccentBlue, { IconUpload(AccentBlue) }, onUploadFile)
            MenuItem("Take photo", "take-photo", AccentGreen, { IconCamera(AccentGreen) }, onTakePhoto)
        }
    }
}

@Composable
private fun MenuItem(
    label: String,
    tag: String,
    accent: Color,
    icon: @Composable () -> Unit,
    onClick: () -> Unit
) {
    Row(
        Modifier
            .fillMaxWidth()
            .height(48.dp)
            .clickable(onClick = onClick)
            .testTag("attach/$tag")
            .padding(horizontal = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp)
    ) {
        Box(
            Modifier.size(32.dp).clip(RoundedCornerShape(10.dp)).background(tinted(accent)),
            contentAlignment = Alignment.Center
        ) { icon() }
        KitText(label, TextPrimary, 14, FontWeight.Medium)
    }
}

@Composable
private fun Header(onMenu: () -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .background(Bg)
            .statusBarsPadding()
            .height(56.dp)
            .padding(horizontal = 16.dp),
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
        Image(
            painter = painterResource(R.drawable.logo_mark),
            contentDescription = "Astrid logo — star with orbit",
            modifier = Modifier.size(24.dp)
        )
        Spacer(Modifier.width(8.dp))
        KitText("Astrid", TextPrimary, 18, FontWeight.Bold)
        Spacer(Modifier.weight(1f))
        Chip("Working", AccentGreen)
    }
}
