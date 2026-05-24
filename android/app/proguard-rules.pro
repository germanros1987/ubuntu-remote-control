# ML Kit barcode scanning loads native + reflection-based components.
-keep class com.google.mlkit.** { *; }
-keep class com.google.android.gms.internal.mlkit_vision_barcode.** { *; }

# CameraX uses reflection for impl selection.
-keep class androidx.camera.** { *; }
