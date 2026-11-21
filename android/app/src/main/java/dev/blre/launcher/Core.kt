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
}
