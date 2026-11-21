package dev.blre.launcher.ui

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

// One seeded palette instead of dynamic colour: dynamic colour needs Android 12
// and would make every screenshot of this app look like a different app. The
// seed is the old launcher's green, kept recognisable on purpose.
private val LightScheme = lightColorScheme(
    primary = Color(0xFF2E6A4F),
    onPrimary = Color(0xFFFFFFFF),
    primaryContainer = Color(0xFFB2F0CE),
    onPrimaryContainer = Color(0xFF00210F),
    secondary = Color(0xFF4F6354),
    onSecondary = Color(0xFFFFFFFF),
    secondaryContainer = Color(0xFFD2E8D6),
    onSecondaryContainer = Color(0xFF0D1F14),
    background = Color(0xFFFBFDF8),
    onBackground = Color(0xFF191C19),
    surface = Color(0xFFFBFDF8),
    onSurface = Color(0xFF191C19),
    surfaceVariant = Color(0xFFDCE5DB),
    onSurfaceVariant = Color(0xFF404943),
    outline = Color(0xFF707973),
)

private val DarkScheme = darkColorScheme(
    primary = Color(0xFF97D5B3),
    onPrimary = Color(0xFF00391F),
    primaryContainer = Color(0xFF135232),
    onPrimaryContainer = Color(0xFFB2F0CE),
    secondary = Color(0xFFB6CCBB),
    onSecondary = Color(0xFF223527),
    secondaryContainer = Color(0xFF384B3D),
    onSecondaryContainer = Color(0xFFD2E8D6),
    background = Color(0xFF101410),
    onBackground = Color(0xFFE1E3DE),
    surface = Color(0xFF101410),
    onSurface = Color(0xFFE1E3DE),
    surfaceVariant = Color(0xFF404943),
    onSurfaceVariant = Color(0xFFC0C9C1),
    outline = Color(0xFF8A938C),
)

@Composable
fun BlreTheme(
    dark: Boolean = isSystemInDarkTheme(),
    content: @Composable () -> Unit,
) {
    MaterialTheme(
        colorScheme = if (dark) DarkScheme else LightScheme,
        content = content,
    )
}
