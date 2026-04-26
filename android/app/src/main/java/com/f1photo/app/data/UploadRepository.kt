package com.f1photo.app.data

import android.content.Context
import android.net.Uri
import androidx.work.Constraints
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkManager
import com.f1photo.app.data.db.UploadQueueDao
import com.f1photo.app.data.db.UploadQueueEntity
import com.f1photo.app.data.work.UploadWorker
import kotlinx.coroutines.flow.Flow
import java.io.File
import java.io.FileOutputStream
import java.util.UUID

class UploadRepository(
    private val context: Context,
    private val dao: UploadQueueDao,
) {
    val all: Flow<List<UploadQueueEntity>> = dao.observeAll()
    val pendingCount: Flow<Int> = dao.pendingCountFlow()

    /**
     * Copy the given URI into app-private storage so the file survives the
     * caller's content provider lifetime, then enqueue a Room row + schedule
     * the upload worker.
     */
    suspend fun enqueueFromUri(
        projectId: String,
        woId: String,
        ownerType: String,
        sourceUri: Uri,
        angle: String = "unknown",
        ownerId: String? = null,
        employeeNo: String? = null,
        sn: String? = null,
    ): Long {
        val cached = copyToCache(sourceUri)
        val row = UploadQueueEntity(
            projectId = projectId,
            woId = woId,
            filePath = cached.absolutePath,
            ownerType = ownerType,
            ownerId = ownerId,
            employeeNo = employeeNo,
            sn = sn,
            angle = angle,
        )
        val id = dao.insert(row)
        scheduleWorker()
        return id
    }

    suspend fun enqueueFromFile(
        projectId: String,
        woId: String,
        ownerType: String,
        file: File,
        angle: String = "unknown",
    ): Long {
        val row = UploadQueueEntity(
            projectId = projectId,
            woId = woId,
            filePath = file.absolutePath,
            ownerType = ownerType,
            angle = angle,
        )
        val id = dao.insert(row)
        scheduleWorker()
        return id
    }

    suspend fun deleteCompleted() = dao.deleteCompleted()
    suspend fun delete(id: Long) = dao.deleteById(id)
    suspend fun retry(id: Long) {
        dao.updateStatus(id, "pending", null, 0)
        scheduleWorker()
    }

    fun scheduleWorker() {
        val constraints = Constraints.Builder()
            .setRequiredNetworkType(NetworkType.CONNECTED)
            .build()
        val request = OneTimeWorkRequestBuilder<UploadWorker>()
            .setConstraints(constraints)
            .build()
        WorkManager.getInstance(context)
            .enqueueUniqueWork(UploadWorker.UNIQUE_NAME, ExistingWorkPolicy.APPEND_OR_REPLACE, request)
    }

    private fun copyToCache(uri: Uri): File {
        val outDir = File(context.cacheDir, "upload_queue").apply { mkdirs() }
        val outFile = File(outDir, "${UUID.randomUUID()}.jpg")
        context.contentResolver.openInputStream(uri).use { input ->
            requireNotNull(input) { "Cannot open URI: $uri" }
            FileOutputStream(outFile).use { output -> input.copyTo(output) }
        }
        return outFile
    }
}
