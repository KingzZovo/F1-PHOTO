package com.f1photo.app.data.api

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

@Serializable
data class LoginRequest(val username: String, val password: String)

@Serializable
data class LoginResponse(
    @SerialName("access_token") val accessToken: String,
    val user: LoginUser,
)

@Serializable
data class LoginUser(
    val id: String,
    val username: String,
    val role: String,
    @SerialName("full_name") val fullName: String? = null,
)

@Serializable
data class WorkOrder(
    val id: String,
    @SerialName("project_id") val projectId: String,
    val code: String,
    val title: String? = null,
    val status: String,
    @SerialName("created_at") val createdAt: String,
    @SerialName("updated_at") val updatedAt: String,
)

@Serializable
data class WorkOrderListResponse(
    val data: List<WorkOrder>,
    val page: Int,
    @SerialName("page_size") val pageSize: Int,
    val total: Int,
)

@Serializable
data class PhotoUploadResponse(
    val id: String,
    val hash: String,
    val status: String,
    val deduped: Boolean,
    @SerialName("work_order_id") val workOrderId: String? = null,
    @SerialName("owner_type") val ownerType: String? = null,
    @SerialName("owner_id") val ownerId: String? = null,
    val angle: String,
)

@Serializable
data class AppErrorBody(val error: AppErrorPayload)

@Serializable
data class AppErrorPayload(val code: String, val message: String)
