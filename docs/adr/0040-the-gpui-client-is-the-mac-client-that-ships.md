# ADR-0040: The GPUI client is the Mac client that ships

The GPUI client in `clients/gpui` is Wheel's only Mac client: `make deploy-client` installs it as `/Applications/Wheel.app` and `make deploy-dev` as `/Applications/Wheel Dev.app`. The iOS app is Swift, and WheelKit, WheelUI and WheelLuau serve it alone (ADR-0056).
