import sys
import os

try:
    from PIL import Image
except ImportError:
    import subprocess
    subprocess.check_call([sys.executable, "-m", "pip", "install", "Pillow"])
    from PIL import Image

def add_macos_padding(input_path, output_path):
    img = Image.open(input_path).convert("RGBA")
    
    # Target canvas size is 1024x1024
    canvas_size = 1024
    
    # macOS HIG: icon usually takes ~82% of the canvas to match native app icon sizing in Dock
    icon_size = int(canvas_size * 0.82)
    
    # Resize the original squircle image (which is edge-to-edge right now)
    img_resized = img.resize((icon_size, icon_size), Image.Resampling.LANCZOS)
    
    # Create transparent canvas
    canvas = Image.new("RGBA", (canvas_size, canvas_size), (0, 0, 0, 0))
    
    # Paste resized icon into center
    offset = (canvas_size - icon_size) // 2
    canvas.paste(img_resized, (offset, offset), img_resized)
    
    # Save the padded icon
    os.makedirs(os.path.dirname(output_path), exist_ok=True)
    canvas.save(output_path, "PNG")
    print(f"Padded icon saved to {output_path}")

input_image = "/Users/batuhanyuksel/Documents/downloadmanager/src-tauri/app-icon.png"
output_image_tauri = "/Users/batuhanyuksel/Documents/downloadmanager/src-tauri/icons/icon.png"
output_image_public = "/Users/batuhanyuksel/Documents/downloadmanager/public/icon.png"
output_image_assets = "/Users/batuhanyuksel/Documents/downloadmanager/src/assets/icon.png"

if os.path.exists(input_image):
    add_macos_padding(input_image, output_image_tauri)
    add_macos_padding(input_image, output_image_public)
    add_macos_padding(input_image, output_image_assets)
else:
    print(f"Input image not found: {input_image}")
