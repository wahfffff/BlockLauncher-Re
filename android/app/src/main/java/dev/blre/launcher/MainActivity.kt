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
    // The library lives in app-private storage: a game APK the user imported is
    // their file, and nothing outside this app needs to read it.
    val root = remember { File(context.filesDir, "library").absolutePath }
    val scope = rememberCoroutineScope()
    val snackbar = remember { SnackbarHostState() }

    var builds by remember { mutableStateOf<List<GameBuild>>(emptyList()) }
    var coreVersion by remember { mutableStateOf("…") }
    var loading by remember { mutableStateOf(true) }
    var fatal by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    var showAbout by remember { mutableStateOf(false) }
    var pendingDelete by remember { mutableStateOf<GameBuild?>(null) }

    fun refresh() {
        scope.launch {
            loading = true
            try {
                coreVersion = Core.version()
                builds = Core.list(root)
                fatal = null
            } catch (t: Throwable) {
                fatal = t.message ?: t.toString()
            } finally {
                loading = false
            }
        }
    }

    LaunchedEffect(root) { refresh() }

    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri: Uri? ->
        if (uri == null) {
            return@rememberLauncherForActivityResult
        }
        scope.launch {
            busy = true
            var staged: File? = null
            try {
                // The picker hands back a URI, and the core reads a path. Copying
                // once into the cache is simpler than teaching the core to read
                // through a content resolver, and it keeps the core testable on a
                // host machine.
                val file = stagePick(context, uri)
                staged = file
                val result = Core.import(root, file.absolutePath)
                builds = Core.list(root)
                snackbar.showSnackbar(
                    if (result.deduped) {
                        "这个安装包已经在版本库里了"
                    } else {
                        "已导入 ${result.build.displayName}"
                    }
                )
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
                onClick = { picker.launch(arrayOf("*/*")) },
                icon = {},
                text = { Text(if (busy) "正在处理…" else "导入 APK") },
            )
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { insets ->
        Box(
            modifier = Modifier
                .padding(insets)
                .fillMaxSize()
        ) {
            when {
                loading -> Centered { CircularProgressIndicator() }

                fatal != null -> Message(
                    title = "核心没有响应",
                    body = fatal!!,
                    action = "重试",
                    onAction = { refresh() },
                )

                builds.isEmpty() -> Message(
                    title = "版本库是空的",
                    body = "导入一个你自己已有的 Minecraft 安装包（APK）。\n" +
                        "这个应用不联网、不提供下载，只管理本地文件。",
                    action = null,
                    onAction = null,
                )

                else -> LazyColumn(
                    contentPadding = PaddingValues(start = 16.dp, top = 12.dp, end = 16.dp, bottom = 96.dp),
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    item {
                        Text(
                            text = "共 ${builds.size} 个版本",
                            style = MaterialTheme.typography.labelLarge,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.padding(bottom = 4.dp),
                        )
                    }
                    items(builds, key = { it.id }) { build ->
                        BuildCard(build = build, onDelete = { pendingDelete = build })
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
                        "这是一个本地启动器：读你已经有的 APK，不下载、不内置游戏文件。",
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
private fun Centered(content: @Composable () -> Unit) {
    Box(modifier = Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        content()
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
        val target = File(context.cacheDir, "import-${System.currentTimeMillis()}.apk")
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
