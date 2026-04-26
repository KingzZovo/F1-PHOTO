package com.f1photo.app.data.db

import androidx.room.ColumnInfo
import androidx.room.Dao
import androidx.room.Database
import androidx.room.Entity
import androidx.room.Insert
import androidx.room.OnConflictStrategy
import androidx.room.PrimaryKey
import androidx.room.Query
import androidx.room.RoomDatabase
import kotlinx.coroutines.flow.Flow

/** Pending uploads queued for the WorkManager worker. */
@Entity(tableName = "upload_queue")
data class UploadQueueEntity(
    @PrimaryKey(autoGenerate = true) val id: Long = 0,
    @ColumnInfo(name = "project_id") val projectId: String,
    @ColumnInfo(name = "wo_id") val woId: String,
    @ColumnInfo(name = "file_path") val filePath: String,
    @ColumnInfo(name = "owner_type") val ownerType: String,
    @ColumnInfo(name = "owner_id") val ownerId: String? = null,
    @ColumnInfo(name = "employee_no") val employeeNo: String? = null,
    @ColumnInfo(name = "sn") val sn: String? = null,
    val angle: String = "unknown",
    /** pending | uploading | done | failed */
    val status: String = "pending",
    @ColumnInfo(name = "error_message") val errorMessage: String? = null,
    @ColumnInfo(name = "attempts") val attempts: Int = 0,
    @ColumnInfo(name = "created_at") val createdAt: Long = System.currentTimeMillis(),
    @ColumnInfo(name = "updated_at") val updatedAt: Long = System.currentTimeMillis(),
)

@Dao
interface UploadQueueDao {
    @Insert(onConflict = OnConflictStrategy.REPLACE)
    suspend fun insert(entity: UploadQueueEntity): Long

    @Query("SELECT * FROM upload_queue ORDER BY created_at DESC")
    fun observeAll(): Flow<List<UploadQueueEntity>>

    @Query("SELECT * FROM upload_queue WHERE status = 'pending' OR status = 'failed' ORDER BY id LIMIT :limit")
    suspend fun nextBatch(limit: Int): List<UploadQueueEntity>

    @Query("UPDATE upload_queue SET status = :status, error_message = :err, attempts = attempts + :attemptsDelta, updated_at = :now WHERE id = :id")
    suspend fun updateStatus(id: Long, status: String, err: String?, attemptsDelta: Int, now: Long = System.currentTimeMillis())

    @Query("DELETE FROM upload_queue WHERE id = :id")
    suspend fun deleteById(id: Long)

    @Query("DELETE FROM upload_queue WHERE status = 'done'")
    suspend fun deleteCompleted()

    @Query("SELECT COUNT(*) FROM upload_queue WHERE status IN ('pending','failed','uploading')")
    fun pendingCountFlow(): Flow<Int>
}

@Database(entities = [UploadQueueEntity::class], version = 1, exportSchema = false)
abstract class F1Database : RoomDatabase() {
    abstract fun uploadQueueDao(): UploadQueueDao
}
