package com.f1photo.app.ui.upload

import android.Manifest
import android.content.Context
import android.net.Uri
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ArrowBack
import androidx.compose.material.icons.filled.PhotoCamera
import androidx.compose.material.icons.filled.PhotoLibrary
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.core.content.FileProvider
import com.f1photo.app.di.ServiceLocator
import kotlinx.coroutines.launch
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

private val OWNER_TYPES = listOf("wo_raw" to "现场原图", "person" to "人员", "tool" to "工具", "device" to "设备")
private val ANGLES = listOf("unknown" to "未知", "front" to "正面", "side" to "侧面", "back" to "背面")

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun UploadScreen(woId: String, onBack: () -> Unit, onOpenQueue: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val repo = ServiceLocator.uploadRepository
    val settings = ServiceLocator.settingsStore
    val projectId by settings.projectIdFlow.collectAsState(initial = "")
    val pending by repo.pendingCount.collectAsState(initial = 0)

    var ownerType by remember { mutableStateOf("wo_raw") }
    var angle by remember { mutableStateOf("unknown") }
    var employeeNo by remember { mutableStateOf("") }
    var sn by remember { mutableStateOf("") }
    var lastMessage by remember { mutableStateOf<String?>(null) }

    val cameraPermLauncher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {}
    val storagePermLauncher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {}

    var pendingCameraUri by remember { mutableStateOf<Uri?>(null) }
    val takePicture = rememberLauncherForActivityResult(ActivityResultContracts.TakePicture()) { ok ->
        val uri = pendingCameraUri
        if (ok && uri != null && projectId.isNotBlank()) {
            scope.launch {
                repo.enqueueFromUri(
                    projectId = projectId,
                    woId = woId,
                    ownerType = ownerType,
                    sourceUri = uri,
                    angle = angle,
                    employeeNo = employeeNo.ifBlank { null },
                    sn = sn.ifBlank { null },
                )
                lastMessage = "已加入上传队列"
            }
        }
    }

    val pickImages = rememberLauncherForActivityResult(
        ActivityResultContracts.GetMultipleContents(),
    ) { uris ->
        if (uris.isEmpty() || projectId.isBlank()) return@rememberLauncherForActivityResult
        scope.launch {
            for (uri in uris) {
                runCatching {
                    repo.enqueueFromUri(
                        projectId = projectId,
                        woId = woId,
                        ownerType = ownerType,
                        sourceUri = uri,
                        angle = angle,
                        employeeNo = employeeNo.ifBlank { null },
                        sn = sn.ifBlank { null },
                    )
                }
            }
            lastMessage = "已加入 ${uris.size} 张到上传队列"
        }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("上传照片") },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.Default.ArrowBack, contentDescription = "返回")
                    }
                },
            )
        },
    ) { padding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(padding)
                .padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text("工单：$woId", style = MaterialTheme.typography.bodySmall)

            Text("所属类型")
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth()) {
                OWNER_TYPES.forEach { (value, label) ->
                    FilterChip(
                        selected = ownerType == value,
                        onClick = { ownerType = value },
                        label = { Text(label) },
                    )
                }
            }
            Text("拍摄角度")
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth()) {
                ANGLES.forEach { (value, label) ->
                    FilterChip(
                        selected = angle == value,
                        onClick = { angle = value },
                        label = { Text(label) },
                    )
                }
            }

            if (ownerType == "person") {
                OutlinedTextField(
                    value = employeeNo,
                    onValueChange = { employeeNo = it },
                    label = { Text("工号提示（可选）") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
            }
            if (ownerType == "tool" || ownerType == "device") {
                OutlinedTextField(
                    value = sn,
                    onValueChange = { sn = it },
                    label = { Text("SN 提示（可选）") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
            }

            Spacer(Modifier.height(8.dp))

            Button(
                onClick = {
                    cameraPermLauncher.launch(Manifest.permission.CAMERA)
                    val (uri, _) = createCameraOutput(context)
                    pendingCameraUri = uri
                    takePicture.launch(uri)
                },
                modifier = Modifier.fillMaxWidth(),
            ) {
                Icon(Icons.Default.PhotoCamera, contentDescription = null)
                Spacer(Modifier.height(4.dp))
                Text("  拍照")
            }
            Button(
                onClick = {
                    val perm = if (Build.VERSION.SDK_INT >= 33)
                        Manifest.permission.READ_MEDIA_IMAGES
                    else Manifest.permission.READ_EXTERNAL_STORAGE
                    storagePermLauncher.launch(perm)
                    pickImages.launch("image/*")
                },
                modifier = Modifier.fillMaxWidth(),
            ) {
                Icon(Icons.Default.PhotoLibrary, contentDescription = null)
                Spacer(Modifier.height(4.dp))
                Text("  从相册选择（可多选）")
            }

            lastMessage?.let { Text(it, color = MaterialTheme.colorScheme.primary) }

            if (pending > 0) {
                Button(onClick = onOpenQueue, modifier = Modifier.fillMaxWidth()) {
                    Text("查看上传队列（$pending）")
                }
            }
        }
    }
}

/** Returns (contentUri, file) for the camera capture target. */
private fun createCameraOutput(context: Context): Pair<Uri, File> {
    val ts = SimpleDateFormat("yyyyMMdd_HHmmss", Locale.US).format(Date())
    val outDir = File(context.cacheDir, "camera_capture").apply { mkdirs() }
    val file = File(outDir, "IMG_$ts.jpg")
    val authority = context.packageName + ".fileprovider"
    val uri = FileProvider.getUriForFile(context, authority, file)
    return uri to file
}
