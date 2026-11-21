package dev.blre.launcher

import android.net.Uri
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ElevatedCard
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Switch
import androidx.compose.material3.Tab
import androidx.compose.material3.TabRow
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import dev.blre.launcher.ui.BlreTheme
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.util.Locale

/** The three things this launcher manages. Each is a separate store. */
private enum class Section(val label: String, val emptyTitle: String, val emptyBody: String) {
    BUILDS(
        "版本",
        "版本库是空的",
        "导入一个你自己已有的 Minecraft 安装包（APK）。\n" +
            "这个应用不联网、不提供下载，只管理本地文件。",
    ),
    PACKS(
        "材质包",
        "还没有材质包",
        "导入一个你自己有的材质包（.zip）。\n" +
            "导入时会检查里面到底有没有贴图文件——空包会在这一步被拒绝，" +
            "而不是等到进游戏才失败。",
    ),
    SCRIPTS(
        "脚本",
        "还没有检查过脚本",
        "选一个 ModPE 脚本（.js），用下面的按钮检查它能不能跑。\n" +
            "现在还没有接入游戏，所以调用 Level、Player 这类函数会如实报告" +
            "“没有连接游戏”，而不是假装成功。",
    ),
}

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            BlreTheme {
                LauncherApp()
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun LauncherApp() {
    val context = LocalContext.current
    // Each store lives in app-private storage: the files the user imported are
    // theirs, and nothing outside this app needs to read them.
    val root = remember { File(context.filesDir, "library").absolutePath }
    val scope = rememberCoroutineScope()
    val snackbar = remember { SnackbarHostState() }

    var tab by remember { mutableStateOf(Section.BUILDS) }
    var builds by remember { mutableStateOf<List<GameBuild>>(emptyList()) }
    var packs by remember { mutableStateOf<List<TexturePack>>(emptyList()) }
    var api by remember { mutableStateOf<ScriptApi?>(null) }
    var scriptName by remember { mutableStateOf<String?>(null) }
    var report by remember { mutableStateOf<ScriptReport?>(null) }
    var coreVersion by remember { mutableStateOf("…") }
    var loading by remember { mutableStateOf(true) }
    var fatal by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    var showAbout by remember { mutableStateOf(false) }
    var pendingDelete by remember { mutableStateOf<GameBuild?>(null) }
    var pendingPackDelete by remember { mutableStateOf<TexturePack?>(null) }

    fun refresh() {
        scope.launch {
            loading = true
            try {
                coreVersion = Core.version()
                builds = Core.list(root)
                packs = Core.packs(root)
                api = Core.scriptApi()
                fatal = null
            } catch (t: Throwable) {
                fatal = t.message ?: t.toString()
            } finally {
                loading = false
            }
        }
    }

    LaunchedEffect(root) { refresh() }

    // Captured at launch: the result arrives later, and the user may have
    // switched tabs in the meantime. Importing a .js as an APK because the tab
    // moved is the kind of bug that only shows up in a race.
    var pickTarget by remember { mutableStateOf(Section.BUILDS) }

    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri: Uri? ->
        if (uri == null) {
            return@rememberLauncherForActivityResult
        }
        val target = pickTarget
        scope.launch {
            busy = true
            var staged: File? = null
            try {
                // The picker hands back a URI, and the core reads paths. Copying
                // once into the cache keeps the core a plain library taking paths,
                // which is what lets it be tested off-device.
                val file = stagePick(context, uri)
                staged = file
                when (target) {
                    Section.BUILDS -> {
                        val result = Core.import(root, file.absolutePath)
                        builds = Core.list(root)
                        snackbar.showSnackbar(
                            if (result.deduped) {
                                "这个安装包已经在版本库里了"
                            } else {
                                "已导入 ${result.build.displayName}"
                            }
                        )
                    }

                    Section.PACKS -> {
                        val result = Core.importPack(root, file.absolutePath, file.name)
                        packs = Core.packs(root)
                        snackbar.showSnackbar(
                            if (result.deduped) {
                                "这个材质包已经在库里了"
                            } else {
                                "已导入 ${result.pack.displayName}（${result.pack.textures} 张贴图）"
                            }
                        )
                    }

                    Section.SCRIPTS -> {
                        val source = withContext(Dispatchers.IO) { file.readText() }
                        scriptName = file.name
                        report = Core.checkScript(source)
                    }
                }
            } catch (t: Throwable) {
                snackbar.showSnackbar(t.message ?: "导入失败")
            } finally {
                staged?.delete()
                busy = false
            }
        }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("BlockLauncher-Re") },
                actions = {
                    TextButton(onClick = { showAbout = true }) { Text("关于") }
                },
            )
        },
        floatingActionButton = {
            ExtendedFloatingActionButton(
                onClick = {
                    pickTarget = tab
                    picker.launch(arrayOf("*/*"))
                },
                icon = {},
                text = {
                    Text(
                        when {
                            busy -> "正在处理…"
                            tab == Section.BUILDS -> "导入 APK"
                            tab == Section.PACKS -> "导入材质包"
                            else -> "检查脚本"
                        }
                    )
                },
            )
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { insets ->
        Column(
            modifier = Modifier
                .padding(insets)
                .fillMaxSize()
        ) {
            TabRow(selectedTabIndex = tab.ordinal) {
                Section.entries.forEach { entry ->
                    Tab(
                        selected = tab == entry,
                        onClick = { tab = entry },
                        text = { Text(entry.label) },
                    )
                }
            }

            Box(modifier = Modifier.weight(1f)) {
                when {
                    loading -> Centered { CircularProgressIndicator() }

                    fatal != null -> Message(
                        title = "核心没有响应",
                        body = fatal!!,
                        action = "重试",
                        onAction = { refresh() },
                    )

                    else -> when (tab) {
                        Section.BUILDS -> BuildsList(builds, tab, onDelete = { pendingDelete = it })

                        Section.PACKS -> PacksList(
                            packs = packs,
                            tab = tab,
                            onDelete = { pendingPackDelete = it },
                            onToggle = { pack, enabled ->
                                scope.launch {
                                    try {
                                        Core.setPackEnabled(root, pack.id, enabled)
                                        packs = Core.packs(root)
                                    } catch (t: Throwable) {
                                        snackbar.showSnackbar(t.message ?: "改不了这个材质包的开关")
                                    }
                                }
                            },
                        )

                        Section.SCRIPTS -> ScriptsPane(tab, api, scriptName, report)
                    }
                }
            }
        }
    }

    pendingDelete?.let { build ->
        AlertDialog(
            onDismissRequest = { pendingDelete = null },
            title = { Text("删除这个版本？") },
            text = {
                Text(
                    "“${build.displayName}”会从版本库中删除，APK 文件也一并删除。" +
                        "你磁盘上原始的安装包不受影响。"
                )
            },
            confirmButton = {
                TextButton(onClick = {
                    val victim = build
                    pendingDelete = null
                    scope.launch {
                        try {
                            Core.remove(root, victim.id)
                            builds = Core.list(root)
                            snackbar.showSnackbar("已删除 ${victim.displayName}")
                        } catch (t: Throwable) {
                            snackbar.showSnackbar(t.message ?: "删除失败")
                        }
                    }
                }) { Text("删除") }
            },
            dismissButton = {
                TextButton(onClick = { pendingDelete = null }) { Text("取消") }
            },
        )
    }

    pendingPackDelete?.let { pack ->
        AlertDialog(
            onDismissRequest = { pendingPackDelete = null },
            title = { Text("删除这个材质包？") },
            text = { Text("“${pack.displayName}”会从库里删除，原始压缩包不受影响。") },
            confirmButton = {
                TextButton(onClick = {
                    val victim = pack
                    pendingPackDelete = null
                    scope.launch {
                        try {
                            Core.removePack(root, victim.id)
                            packs = Core.packs(root)
                            snackbar.showSnackbar("已删除 ${victim.displayName}")
                        } catch (t: Throwable) {
                            snackbar.showSnackbar(t.message ?: "删除失败")
                        }
                    }
                }) { Text("删除") }
            },
            dismissButton = {
                TextButton(onClick = { pendingPackDelete = null }) { Text("取消") }
            },
        )
    }

    if (showAbout) {
        AlertDialog(
            onDismissRequest = { showAbout = false },
            title = { Text("关于") },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text("Rust 核心 $coreVersion")
                    Text(
                        "版本库位置\n$root",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    Spacer(Modifier.height(4.dp))
                    Text(
                        "这是一个本地启动器：读你已经有的 APK 和材质包，不下载、不内置游戏文件。",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            },
            confirmButton = {
                TextButton(onClick = { showAbout = false }) { Text("好") }
            },
        )
    }
}

// ------------------------------------------------------------------- lists

@Composable
private fun BuildsList(builds: List<GameBuild>, tab: Section, onDelete: (GameBuild) -> Unit) {
    if (builds.isEmpty()) {
        Message(tab.emptyTitle, tab.emptyBody, action = null, onAction = null)
        return
    }
    LazyColumn(
        contentPadding = PaddingValues(start = 16.dp, top = 12.dp, end = 16.dp, bottom = 96.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        item { ListHeader("共 ${builds.size} 个版本") }
        items(builds, key = { it.id }) { build ->
            BuildCard(build = build, onDelete = { onDelete(build) })
        }
    }
}

@Composable
private fun PacksList(
    packs: List<TexturePack>,
    tab: Section,
    onDelete: (TexturePack) -> Unit,
    onToggle: (TexturePack, Boolean) -> Unit,
) {
    if (packs.isEmpty()) {
        Message(tab.emptyTitle, tab.emptyBody, action = null, onAction = null)
        return
    }
    LazyColumn(
        contentPadding = PaddingValues(start = 16.dp, top = 12.dp, end = 16.dp, bottom = 96.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        item {
            ListHeader(
                "共 ${packs.size} 个材质包 · ${packs.count { it.enabled }} 个已启用"
            )
        }
        items(packs, key = { it.id }) { pack ->
            PackCard(pack = pack, onDelete = { onDelete(pack) }, onToggle = { onToggle(pack, it) })
        }
    }
}

@Composable
private fun ListHeader(text: String) {
    Text(
        text = text,
        style = MaterialTheme.typography.labelLarge,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(bottom = 4.dp),
    )
}

@Composable
private fun BuildCard(build: GameBuild, onDelete: () -> Unit) {
    ElevatedCard(modifier = Modifier.fillMaxWidth()) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(start = 16.dp, top = 14.dp, end = 8.dp, bottom = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column(modifier = Modifier.weight(1f)) {
                Text(build.displayName, style = MaterialTheme.typography.titleMedium)
                Text(
                    text = "${build.packageName} · ${build.versionName} (${build.versionCode})",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Text(
                    text = "${build.abiSummary} · ${humanSize(build.size)}",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Text(
                    text = build.sha256.take(16) + "…",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.outline,
                )
            }
            TextButton(onClick = onDelete) { Text("删除") }
        }
    }
}

@Composable
private fun PackCard(pack: TexturePack, onDelete: () -> Unit, onToggle: (Boolean) -> Unit) {
    ElevatedCard(modifier = Modifier.fillMaxWidth()) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(start = 16.dp, top = 14.dp, end = 8.dp, bottom = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column(modifier = Modifier.weight(1f)) {
                Text(pack.displayName, style = MaterialTheme.typography.titleMedium)
                Text(
                    text = listOfNotNull(
                        pack.version.takeIf { it.isNotBlank() }?.let { "v$it" },
                        "${pack.textures} 张贴图",
                        humanSize(pack.size),
                    ).joinToString(" · "),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                if (pack.description.isNotBlank()) {
                    Text(
                        text = pack.description,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                Text(
                    text = pack.id.take(16) + "…",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.outline,
                )
            }
            Switch(checked = pack.enabled, onCheckedChange = onToggle)
            TextButton(onClick = onDelete) { Text("删除") }
        }
    }
}

// ----------------------------------------------------------------- scripts

@Composable
private fun ScriptsPane(
    tab: Section,
    api: ScriptApi?,
    scriptName: String?,
    report: ScriptReport?,
) {
    LazyColumn(
        contentPadding = PaddingValues(start = 16.dp, top = 12.dp, end = 16.dp, bottom = 96.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        item {
            if (report == null) {
                // Inline rather than the full-screen `Message`: this pane still
                // has the API list below it, so the empty state is a paragraph,
                // not a whole screen.
                InlineNotice(tab.emptyTitle, tab.emptyBody)
            } else {
                ReportCard(name = scriptName, report = report)
            }
        }

        if (api != null) {
            item { ListHeader("脚本可以调用这些（核心实际提供的）") }
            // Grouped by namespace so the list reads as an API rather than as a
            // wall of names; a free function is its own group.
            val groups = api.globals.groupBy {
                if ('.' in it.name) it.name.substringBefore('.') else "全局函数"
            }
            items(groups.keys.sorted(), key = { it }) { key ->
                val members = groups[key].orEmpty()
                ElevatedCard(modifier = Modifier.fillMaxWidth()) {
                    Column(
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(16.dp),
                        verticalArrangement = Arrangement.spacedBy(4.dp),
                    ) {
                        Text(key, style = MaterialTheme.typography.titleSmall)
                        Text(
                            text = members.joinToString("、") { it.name.substringAfterLast('.') },
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
            if (api.hooks.isNotEmpty()) {
                item {
                    Text(
                        text = "脚本可以定义这些钩子：${api.hooks.joinToString("、")}",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        }
    }
}

@Composable
private fun ReportCard(name: String?, report: ScriptReport) {
    ElevatedCard(modifier = Modifier.fillMaxWidth()) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Text(name ?: "脚本", style = MaterialTheme.typography.titleMedium)
            Text(
                text = when {
                    report.halted -> "被运行预算中断——脚本可能永远不会结束"
                    report.failed > 0 -> "运行失败"
                    report.ran > 0 -> "语法和顶层代码没有问题"
                    else -> "没有运行"
                },
                style = MaterialTheme.typography.bodyMedium,
                color = if (report.failed > 0 || report.halted) {
                    MaterialTheme.colorScheme.error
                } else {
                    MaterialTheme.colorScheme.primary
                },
            )
            report.messages.forEach { line ->
                Text(
                    text = line,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            if (!report.halted && report.failed == 0) {
                Text(
                    text = "注意：这里只验证了加载阶段。真正运行时它调用的游戏函数" +
                        "需要接上游戏之后才有效。",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.outline,
                )
            }
        }
    }
}

// ------------------------------------------------------------------ shared

@Composable
private fun Centered(content: @Composable () -> Unit) {
    Box(modifier = Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        content()
    }
}

@Composable
private fun InlineNotice(title: String, body: String) {
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(title, style = MaterialTheme.typography.titleMedium)
        Text(
            body,
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@Composable
private fun Message(
    title: String,
    body: String,
    action: String?,
    onAction: (() -> Unit)?,
) {
    Box(modifier = Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Column(
            modifier = Modifier.padding(32.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text(title, style = MaterialTheme.typography.titleLarge, textAlign = TextAlign.Center)
            Spacer(Modifier.height(12.dp))
            Text(
                body,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
            )
            if (action != null && onAction != null) {
                Spacer(Modifier.height(16.dp))
                TextButton(onClick = onAction) { Text(action) }
            }
        }
    }
}

/**
 * Copy the picked document into the cache and return the file.
 *
 * The copy is the price of the core being a plain library that takes paths: it
 * keeps `Library::import` readable and testable off-device.
 */
private suspend fun stagePick(context: android.content.Context, uri: Uri): File =
    withContext(Dispatchers.IO) {
        val target = File(context.cacheDir, "import-${System.currentTimeMillis()}.bin")
        val input = context.contentResolver.openInputStream(uri)
            ?: throw CoreFailure("打不开所选文件——可能它已经被移走了")
        input.use { src -> target.outputStream().use { dst -> src.copyTo(dst) } }
        target
    }

private fun humanSize(bytes: Long): String = when {
    bytes <= 0 -> "大小未知"
    bytes >= 1048576 -> String.format(Locale.US, "%.1f MiB", bytes / 1048576.0)
    else -> String.format(Locale.US, "%d KiB", bytes / 1024)
}
