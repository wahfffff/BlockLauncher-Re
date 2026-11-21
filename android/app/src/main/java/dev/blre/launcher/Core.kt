package dev.blre.launcher

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject

/** Something the core refused, carrying the core's own message. */
class CoreFailure(message: String) : Exception(message)

/**
 * One build in the library, as the core describes it.
 *
 * The fields mirror `core::catalog::VersionEntry` exactly. That is a deliberate
 * duplication of a shape rather than a second source of truth: the moment this
 * class starts computing things the core does not, the two disagree.
 */
data class GameBuild(
    val id: String,
    val packageName: String,
    val versionName: String,
    val versionCode: Long,
    val abis: List<String>,
    val sha256: String,
    val size: Long,
    val label: String?,
    val importedAt: Long,
) {
    val displayName: String
        get() = label?.takeIf { it.isNotBlank() } ?: "Minecraft $versionName"

    val abiSummary: String
        get() = if (abis.isEmpty()) "无原生代码" else abis.joinToString(" · ")
}

data class ImportResult(val build: GameBuild, val deduped: Boolean)

/**
 * One texture pack, mirroring `core::pack::Pack`.
 *
 * `textures` is the count the core found inside the archive, not an estimate:
 * it is what makes "this pack is empty" visible before the game refuses it.
 */
data class TexturePack(
    val id: String,
    val name: String,
    val version: String,
    val description: String,
    val size: Long,
    val textures: Int,
    val shaders: Int,
    /** `"texture"` or `"shader"`; the core decides, the UI only displays it. */
    val kind: String,
    val enabled: Boolean,
) {
    val displayName: String
        get() = name.takeIf { it.isNotBlank() } ?: "未命名材质包"

    /**
     * What this pack replaces, in one phrase.
     *
     * A shader pack carries no images at all, so a card that only ever said
     * "0 张贴图" would describe a working pack as an empty one.
     */
    val summary: String
        get() = if (kind == "shader") "$shaders 个着色器" else "$textures 张贴图"
}

data class PackImportResult(val pack: TexturePack, val deduped: Boolean)

/**
 * One mod the core found on disk, mirroring `core::mods::Entry`.
 *
 * `usable` is the field that matters: a legacy `.mod` can be *read* but not
 * *applied*, and the row has to say so. A list that showed it like any other
 * mod would send the user looking for a bug in the launcher instead of reading
 * the reason.
 */
data class ModEntry(
    val fileName: String,
    val kind: String,
    val title: String,
    val author: String,
    val usable: Boolean,
    val reason: String,
    val detail: String,
) {
    val displayName: String
        get() = title.takeIf { it.isNotBlank() } ?: fileName

    val kindLabel: String
        get() = if (kind == "native") "原生 mod" else "旧版补丁"
}

/**
 * What running a script produced.
 *
 * `halted` is separate from `failed` on purpose: a script stopped by the
 * execution budget did not have a bug in the usual sense, it ran too long, and
 * saying "stopped" instead of "failed" is the difference between a user
 * deleting the mod and a user looking at its loop.
 */
data class ScriptReport(
    val ran: Int,
    val failed: Int,
    val halted: Boolean,
    val messages: List<String>,
)

/** One callable the script runtime provides. */
data class ScriptGlobal(val name: String, val arity: Int)

/** The script surface, as the runtime itself publishes it. */
data class ScriptApi(val globals: List<ScriptGlobal>, val hooks: List<String>)

/**
 * Typed wrapper over the bridge.
 *
 * Every method runs on [Dispatchers.IO]: the core reads and hashes a whole APK
 * (tens of megabytes), and doing that on the main thread is how a launcher gets
 * an ANR on its first screen.
 */
object Core {

    private fun call(request: String): JSONObject {
        val raw: String? = try {
            NativeBridge.call(request)
        } catch (e: UnsatisfiedLinkError) {
            // The one failure worth its own message: it means the APK was built
            // without the Rust library, which is a packaging bug rather than a
            // user problem.
            throw CoreFailure("APK 里没有 Rust 核心（libblre_core.so 缺失），这个安装包不完整")
        }

        if (raw == null) throw CoreFailure("核心没有返回内容（内存不足？）")

        val envelope = try {
            JSONObject(raw)
        } catch (e: Exception) {
            throw CoreFailure("核心返回了无法解析的内容：${raw.take(120)}")
        }

        if (!envelope.optBoolean("ok", false)) {
            val error = envelope.optJSONObject("error")
            val msg = error?.optString("msg").orEmpty().ifBlank { "核心报告了一个没有说明的错误" }
            throw CoreFailure(msg)
        }

        return envelope.optJSONObject("data") ?: JSONObject()
    }

    private fun parseBuild(o: JSONObject): GameBuild {
        val abis = o.optJSONArray("abis")?.let { arr ->
            (0 until arr.length()).map { arr.getString(it) }
        } ?: emptyList()

        return GameBuild(
            id = o.getString("id"),
            packageName = o.optString("package"),
            versionName = o.optString("version_name"),
            versionCode = o.optLong("version_code"),
            abis = abis,
            sha256 = o.optString("sha256"),
            size = o.optLong("size"),
            label = o.optString("label").takeIf { it.isNotBlank() },
            importedAt = o.optLong("imported_at"),
        )
    }

    suspend fun version(): String = withContext(Dispatchers.IO) {
        call("""{"cmd":"core.version"}""").optString("version", "unknown")
    }

    suspend fun list(root: String): List<GameBuild> = withContext(Dispatchers.IO) {
        val data = call(
            JSONObject().put("cmd", "library.list").put("root", root).toString()
        )
        val arr = data.optJSONArray("versions") ?: return@withContext emptyList()
        (0 until arr.length()).map { parseBuild(arr.getJSONObject(it)) }
    }

    suspend fun import(root: String, path: String): ImportResult = withContext(Dispatchers.IO) {
        val data = call(
            JSONObject()
                .put("cmd", "library.import")
                .put("root", root)
                .put("path", path)
                .toString()
        )
        ImportResult(
            build = parseBuild(data.getJSONObject("entry")),
            deduped = data.optBoolean("deduped", false),
        )
    }

    suspend fun remove(root: String, id: String): GameBuild = withContext(Dispatchers.IO) {
        val data = call(
            JSONObject()
                .put("cmd", "library.remove")
                .put("root", root)
                .put("id", id)
                .toString()
        )
        parseBuild(data.getJSONObject("removed"))
    }

    // ------------------------------------------------------------------ packs

    private fun parsePack(o: JSONObject): TexturePack = TexturePack(
        id = o.getString("id"),
        name = o.optString("name"),
        version = o.optString("version"),
        description = o.optString("description"),
        size = o.optLong("size"),
        textures = o.optInt("textures"),
        shaders = o.optInt("shaders"),
        // Absent from an index the core wrote before shader packs existed.
        kind = o.optString("kind", "texture"),
        enabled = o.optBoolean("enabled", false),
    )

    suspend fun packs(root: String): List<TexturePack> = withContext(Dispatchers.IO) {
        val data = call(JSONObject().put("cmd", "packs.list").put("root", root).toString())
        val arr = data.optJSONArray("packs") ?: return@withContext emptyList()
        (0 until arr.length()).map { parsePack(arr.getJSONObject(it)) }
    }

    suspend fun importPack(root: String, path: String, name: String?): PackImportResult =
        withContext(Dispatchers.IO) {
            val req = JSONObject()
                .put("cmd", "packs.import")
                .put("root", root)
                .put("path", path)
            if (name != null) req.put("name", name)
            val data = call(req.toString())
            PackImportResult(
                pack = parsePack(data.getJSONObject("pack")),
                deduped = data.optBoolean("deduped", false),
            )
        }

    suspend fun removePack(root: String, id: String): Boolean = withContext(Dispatchers.IO) {
        val data = call(
            JSONObject().put("cmd", "packs.remove").put("root", root).put("id", id).toString()
        )
        data.optBoolean("removed", false)
    }

    suspend fun setPackEnabled(root: String, id: String, enabled: Boolean): TexturePack =
        withContext(Dispatchers.IO) {
            val data = call(
                JSONObject()
                    .put("cmd", "packs.enable")
                    .put("root", root)
                    .put("id", id)
                    .put("enabled", enabled)
                    .toString()
            )
            parsePack(data.getJSONObject("pack"))
        }

    // ---------------------------------------------------------------- scripts

    suspend fun scriptApi(): ScriptApi = withContext(Dispatchers.IO) {
        val data = call("""{"cmd":"scripts.api"}""")
        val globals = data.optJSONArray("globals")?.let { arr ->
            (0 until arr.length()).map { i ->
                val o = arr.getJSONObject(i)
                ScriptGlobal(o.optString("name"), o.optInt("arity"))
            }
        } ?: emptyList()
        val hooks = data.optJSONArray("hooks")?.let { arr ->
            (0 until arr.length()).map { arr.getString(it) }
        } ?: emptyList()
        ScriptApi(globals, hooks)
    }

    suspend fun checkScript(source: String): ScriptReport = withContext(Dispatchers.IO) {
        val data = call(
            JSONObject().put("cmd", "scripts.check").put("source", source).toString()
        )
        val messages = data.optJSONArray("messages")?.let { arr ->
            (0 until arr.length()).map { arr.getString(it) }
        } ?: emptyList()
        ScriptReport(
            ran = data.optInt("ran"),
            failed = data.optInt("failed"),
            halted = data.optBoolean("halted", false),
            messages = messages,
        )
    }

    // ------------------------------------------------------------------ mods

    private fun parseMod(o: JSONObject): ModEntry = ModEntry(
        fileName = o.optString("file_name"),
        kind = o.optString("kind", "legacy"),
        title = o.optString("title"),
        author = o.optString("author"),
        usable = o.optBoolean("usable", false),
        reason = o.optString("reason"),
        detail = o.optString("detail"),
    )

    suspend fun scanMods(dir: String): List<ModEntry> = withContext(Dispatchers.IO) {
        val data = call(JSONObject().put("cmd", "mods.scan").put("dir", dir).toString())
        val arr = data.optJSONArray("mods") ?: return@withContext emptyList()
        (0 until arr.length()).map { parseMod(arr.getJSONObject(it)) }
    }
}
