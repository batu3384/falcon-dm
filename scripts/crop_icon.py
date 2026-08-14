import sys
import os

try:
    from PIL import Image, ImageDraw, ImageFilter
except ImportError:
    import subprocess
    subprocess.check_call([sys.executable, "-m", "pip", "install", "Pillow"])
    from PIL import Image, ImageDraw, ImageFilter

def crop_center_squircle(input_path, output_path):
    img = Image.open(input_path).convert("RGBA")
    width, height = img.size
    
    # Assume the mockup has the icon in the exact center taking up roughly 60% of the image
    # We will crop exactly the center 55% to be safe and avoid desktop background
    crop_size = int(min(width, height) * 0.55)
    left = (width - crop_size) // 2
    top = (height - crop_size) // 2
    right = left + crop_size
    bottom = top + crop_size
    
    # Crop to the center square
    img_cropped = img.crop((left, top, right, bottom))
    
    # Create a squircle mask (rounded rectangle)
    # Apple HIG corner radius is roughly 22.5% of the icon size
    mask = Image.new("L", (crop_size, crop_size), 0)
    draw = ImageDraw.Draw(mask)
    radius = int(crop_size * 0.225)
    draw.rounded_rectangle((0, 0, crop_size, crop_size), radius=radius, fill=255)
    
    # Anti-aliasing / smooth edges
    mask = mask.filter(ImageFilter.GaussianBlur(1))
    
    # Apply mask
    img_cropped.putalpha(mask)
    
    # Save as PNG
    img_cropped.save(output_path, "PNG")
    print(f"Cropped successfully to {output_path}")

input_image = "/Users/batuhanyuksel/.gemini/antigravity/brain/5efd6309-96ee-4a4d-839d-fbe76caeab0b/falcon_icon_corporate_v2_cnc_1785423480597.jpg"
output_image = "/Users/batuhanyuksel/Documents/downloadmanager/src-tauri/app-icon.png"

if os.path.exists(input_image):
    crop_center_squircle(input_image, output_image)
else:
    print("Input image not found!")
