package com.f1photo.app

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController
import com.f1photo.app.di.ServiceLocator
import com.f1photo.app.ui.login.LoginScreen
import com.f1photo.app.ui.queue.QueueScreen
import com.f1photo.app.ui.settings.SettingsScreen
import com.f1photo.app.ui.theme.F1PhotoTheme
import com.f1photo.app.ui.upload.UploadScreen
import com.f1photo.app.ui.workorders.WorkOrdersScreen

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            F1PhotoTheme {
                AppNav()
            }
        }
    }
}

private object Routes {
    const val Splash = "splash"
    const val Login = "login"
    const val WorkOrders = "work-orders"
    const val Upload = "upload/{woId}"
    const val Queue = "queue"
    const val Settings = "settings"
    fun upload(woId: String) = "upload/$woId"
}

@Composable
private fun AppNav() {
    val nav = rememberNavController()
    val auth = ServiceLocator.authStore
    val token by auth.tokenFlow.collectAsState(initial = null)
    val start = if (token.isNullOrBlank()) Routes.Login else Routes.WorkOrders

    NavHost(navController = nav, startDestination = start) {
        composable(Routes.Login) {
            LoginScreen(
                onLoggedIn = {
                    nav.navigate(Routes.WorkOrders) {
                        popUpTo(Routes.Login) { inclusive = true }
                    }
                },
                onOpenSettings = { nav.navigate(Routes.Settings) },
            )
        }
        composable(Routes.WorkOrders) {
            WorkOrdersScreen(
                onOpenUpload = { woId -> nav.navigate(Routes.upload(woId)) },
                onOpenQueue = { nav.navigate(Routes.Queue) },
                onOpenSettings = { nav.navigate(Routes.Settings) },
                onLogout = {
                    nav.navigate(Routes.Login) {
                        popUpTo(0) { inclusive = true }
                    }
                },
            )
        }
        composable(Routes.Upload) { backStackEntry ->
            val woId = backStackEntry.arguments?.getString("woId").orEmpty()
            UploadScreen(
                woId = woId,
                onBack = { nav.popBackStack() },
                onOpenQueue = { nav.navigate(Routes.Queue) },
            )
        }
        composable(Routes.Queue) {
            QueueScreen(onBack = { nav.popBackStack() })
        }
        composable(Routes.Settings) {
            SettingsScreen(onBack = { nav.popBackStack() })
        }
        composable(Routes.Splash) {
            Box(modifier = Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                CircularProgressIndicator()
            }
        }
    }
}
