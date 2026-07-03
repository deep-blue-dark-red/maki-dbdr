APP := demoapp
DIST := dist

.PHONY: build test deploy clean

build:
	mkdir -p $(DIST)
	cp -r src/* $(DIST)/

test:
	python3 -m unittest discover -q

deploy: build
	rsync -az $(DIST)/ server-a:/srv/app
	ssh server-a 'systemctl restart demoapp'

clean:
	rm -rf $(DIST)
