import hashlib

f = "verification/esp32c5/artifacts.toml"
s = open(f).read()
src = """
# ESP-IDF sources of the IEEE 802.15.4 driver and the ESP32-C5 register
# headers. Fetched file by file, so `target/vendor/esp-idf/<revision>/`
# mirrors the checkout's layout.
[[source]]
id = "esp-idf"
kind = "git"
repository = "https://github.com/espressif/esp-idf"
revision = "4d59230ddff16327812782151ef0afef202dc6d7"
"""
assert 'id = "esp-idf"' not in s
s = s.replace("\n[[artifact]]", src + "\n[[artifact]]", 1)
for p in open("target/c5-research/idf-paths.txt").read().split():
    h = hashlib.sha256(open("target/c5-research/idfpin/" + p, "rb").read()).hexdigest()
    s += f'\n[[artifact]]\nid = "esp-idf/{p}"\nsource = "esp-idf"\npath = "{p}"\nsha256 = "{h}"\n'
open(f, "w").write(s)
