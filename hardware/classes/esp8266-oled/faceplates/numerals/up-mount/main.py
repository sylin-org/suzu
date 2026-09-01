# Faceplate bootstrap: this firmware automatically runs main.py;
# face.mpy contains precompiled faceplate code so the 13 KB source
# is not compiled in the ESP8266.s 80 KB heap at boot.
import face
