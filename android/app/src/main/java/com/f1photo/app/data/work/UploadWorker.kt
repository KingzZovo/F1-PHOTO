package com.f1photo.app.data.work

import android.content.Context
import androidx.work.CoroutineWorker
import androidx.work.WorkerParameters
import com.f1photo.app.data.db.UploadQueueDao
import com.f1photo.app.data.db.UploadQueueEntity
import com.f1photo.app.di.ServiceLocator
import okhttp3.MediaType.Companion.toMediaTypeOrNull
import okhttp3.MultipartBody
import okhttp3.RequestBody
import okhttp3.RequestBody.Companion.asRequestBody
import okhttp3.RequestBody.Companion.toRequestBody
import java.io.File

/**
 * Drains the upload queue. Each iteration grabs a small batch of pending rows,
 * uploads each, and updates row status. WorkManager will retry the unique work
 * ("upload-queue") with backoff if we return Result.retry().
 */
class UploadWorker(
    appContext: Context,
    params: WorkerParameters,
) : CoroutineWorker(appContext, params) {

    override suspend fun doWork(): Result {
        val dao: UploadQueueDao = ServiceLocator.uploadQueueDao
        val api = ServiceLocator.network.api()

        val batch = dao.nextBatch(BATCH_SIZE)
        if (batch.isEmpty()) return Result.success()

        var anyFailed = false
        for (row in batch) {
            dao.updateStatus(row.id, "uploading", null, 0)
            val ok = runCatching { uploadOne(api, row) }.getOrElse { err ->
                dao.updateStatus(row.id, "failed", err.message, 1)
                anyFailed = true
                false
            }
            if (ok) {
                dao.updateStatus(row.id, "done", null, 1)
                runCatching { File(row.filePath).delete() }
            }
        }

        // Drain any remaining pending rows by re-enqueuing.
        val moreLeft = dao.nextBatch(1).isNotEmpty()
        return when {
            moreLeft && !anyFailed -> Result.success().also { reschedule() }
            anyFailed -> Result.retry()
            else -> Result.success()
        }
    }

    private suspend fun uploadOne(
        api: com.f1photo.app.data.api.F1Api,
        row: UploadQueueEntity,
    ): Boolean {
        val file = File(row.filePath)
        if (!file.exists()) return false
        val mediaType = "image/*".toMediaTypeOrNull()
        val filePart = MultipartBody.Part.createFormData(
            name = "file",
            filename = file.name,
            body = file.asRequestBody(mediaType),
        )
        val text = "text/plain".toMediaTypeOrNull()
        val resp = api.uploadPhoto(
            projectId = row.projectId,
            file = filePart,
            ownerType = row.ownerType.toRequestBody(text),
            woId = row.woId.toRequestBody(text),
            woCode = null,
            ownerId = row.ownerId?.let { it.toRequestBody(text) },
            employeeNo = row.employeeNo?.let { it.toRequestBody(text) },
            sn = row.sn?.let { it.toRequestBody(text) },
            angle = row.angle.toRequestBody(text),
        )
        // Server returns 202 Accepted on dedupe + new uploads.
        return resp.code() in 200..299
    }

    private fun reschedule() {
        ServiceLocator.uploadRepository.scheduleWorker()
    }

    companion object {
        const val UNIQUE_NAME = "upload-queue"
        private const val BATCH_SIZE = 5
    }
}

@Suppress("unused")
private fun stringRequestBody(value: String): RequestBody =
    value.toRequestBody("text/plain".toMediaTypeOrNull())
