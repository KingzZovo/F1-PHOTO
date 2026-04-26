package com.f1photo.app.ui.login

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import com.f1photo.app.data.api.LoginRequest
import com.f1photo.app.di.ServiceLocator
import kotlinx.coroutines.launch

@Composable
fun LoginScreen(onLoggedIn: () -> Unit, onOpenSettings: () -> Unit) {
    val auth = ServiceLocator.authStore
    val network = ServiceLocator.network
    val scope = rememberCoroutineScope()

    var username by remember { mutableStateOf("admin") }
    var password by remember { mutableStateOf("") }
    var loading by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }

    // If a previous JWT is already present, skip the form.
    LaunchedEffect(Unit) {
        if (!auth.token.isNullOrBlank()) onLoggedIn()
    }

    Scaffold { padding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(padding)
                .padding(24.dp),
            verticalArrangement = Arrangement.Center,
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text("F1 Photo", style = androidx.compose.material3.MaterialTheme.typography.headlineMedium)
            Spacer(Modifier.height(24.dp))

            OutlinedTextField(
                value = username,
                onValueChange = { username = it },
                label = { Text("账号") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
            Spacer(Modifier.height(12.dp))
            OutlinedTextField(
                value = password,
                onValueChange = { password = it },
                label = { Text("密码") },
                singleLine = true,
                visualTransformation = PasswordVisualTransformation(),
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                modifier = Modifier.fillMaxWidth(),
            )
            Spacer(Modifier.height(8.dp))
            error?.let {
                Text(it, color = androidx.compose.material3.MaterialTheme.colorScheme.error)
                Spacer(Modifier.height(8.dp))
            }
            Button(
                onClick = {
                    if (loading) return@Button
                    loading = true
                    error = null
                    scope.launch {
                        try {
                            val resp = network.api().login(LoginRequest(username.trim(), password))
                            auth.saveToken(resp.accessToken, resp.user.username)
                            onLoggedIn()
                        } catch (e: Throwable) {
                            error = e.message ?: "登录失败"
                        } finally {
                            loading = false
                        }
                    }
                },
                enabled = !loading && username.isNotBlank() && password.isNotBlank(),
                modifier = Modifier.fillMaxWidth(),
            ) {
                if (loading) CircularProgressIndicator(modifier = Modifier.height(20.dp))
                else Text("登录")
            }
            Spacer(Modifier.height(8.dp))
            TextButton(onClick = onOpenSettings, modifier = Modifier.fillMaxWidth()) {
                Text("服务器设置")
            }
        }
    }
}
