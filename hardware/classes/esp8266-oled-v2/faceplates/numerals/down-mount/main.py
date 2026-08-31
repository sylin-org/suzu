# suzu face bootstrap — this firmware auto-runs only main.py;
# the face itself ships as bytecode (face.mpy) so its 13 KB source
# never compiles on the ESP8266's 80 KB heap at boot.
import face
