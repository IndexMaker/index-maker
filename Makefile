####
# Deployment Script
#
# 1. Build Docker image from currently checked out sources
# 2. Prepare files for deployment in .deploy directory
# 3. Copy files to staging host using SSH_USER
# 4. Launch service remotely on staging host
#
# We build index-maker from source and create Docker image for AMD64 based off Alpine/Linux, and
# then on staging host we build OTLP image, and we launch docker containers with compose.
# 
#

IMAGE_NAME?=index-maker-quote-server

TEMPLATES_DIR?=deploy_templates
DEPLOY_DIR?=.deploy
REMOTE_DIR?=staging

DOCKER_IMAGE_NAME=$(IMAGE_NAME):$(IMAGE_VERSION)
TAR_ARCHIVE_NAME=$(IMAGE_NAME)-$(IMAGE_VERSION).tar


.PHONY: \
	check_ssh_env \
	check_image_version_env \
	check_elastic_env \
	build_docker_image \
	save_docker_image \
	docker_compose_yaml \
	otel_collector_config_yaml \
	dockerfile_oltp \
	build_service_sh \
	stop_service_sh \
	copy_to_remote \
	run_build_service \
	run_stop_service \
	all \
	check_env \
	build \
	prepare \
	deploy \
	clean \
	purge


all: \
	check_env \
	build \
	prepare \
	deploy


check_env: \
	check_ssh_env \
	check_image_version_env \
	check_elastic_env

build: \
	build_docker_image

prepare: \
	clean \
	save_docker_image \
	docker_compose_yaml \
	otel_collector_config_yaml \
	build_service_sh \
	stop_service_sh \
	dockerfile_oltp

deploy: \
	copy_to_remote \
	run_build_service
	@echo "Deployed, you can check logs with 'docker-compose -f docker-compose.prod.yaml logs'"

clean:
	rm -rf $(DEPLOY_DIR)

purge: clean
	docker image rm $(DOCKER_IMAGE_NAME)


check_ssh_env:
ifndef SSH_USER
	$(error SSH_USER is undefined)
endif

check_image_version_env:
ifndef IMAGE_VERSION
	$(error IMAGE_VERSION is undefined)
endif

check_elastic_env:
ifndef ELASTIC_API_KEY
	$(error ELASTIC_API_KEY is undefined)
endif


deploy_dir: check_image_version_env
	mkdir -p $(DEPLOY_DIR)

build_docker_image: check_image_version_env
	docker buildx build --platform linux/amd64 -t $(DOCKER_IMAGE_NAME) .

save_docker_image: deploy_dir
	docker save -o $(DEPLOY_DIR)/$(TAR_ARCHIVE_NAME) $(DOCKER_IMAGE_NAME)

docker_compose_yaml: deploy_dir
	cat $(TEMPLATES_DIR)/docker-compose.prod.yaml.template \
	| sed -e "s/<DOCKER_IMAGE_NAME>/$(DOCKER_IMAGE_NAME)/g" \
	> $(DEPLOY_DIR)/docker-compose.prod-$(IMAGE_VERSION).yaml

otel_collector_config_yaml: deploy_dir check_elastic_env
	cat $(TEMPLATES_DIR)/otel-collector-config.prod.yaml.template \
	| sed -e "s/<ELASTIC_API_KEY>/$(ELASTIC_API_KEY)/g" \
	> $(DEPLOY_DIR)/otel-collector-config.prod.yaml

build_service_sh: deploy_dir
	cat $(TEMPLATES_DIR)/build-service.sh.template \
	| sed -e "s/<IMAGE_VERSION>/$(IMAGE_VERSION)/g" \
	| sed -e "s/<TAR_ARCHIVE_NAME>/$(TAR_ARCHIVE_NAME)/g" \
	> $(DEPLOY_DIR)/build-service-$(IMAGE_VERSION).sh
	chmod a+x $(DEPLOY_DIR)/build-service-$(IMAGE_VERSION).sh

stop_service_sh: deploy_dir
	cat $(TEMPLATES_DIR)/stop-service.sh.template \
	| sed -e "s/<IMAGE_VERSION>/$(IMAGE_VERSION)/g" \
	> $(DEPLOY_DIR)/stop-service-$(IMAGE_VERSION).sh
	chmod a+x $(DEPLOY_DIR)/stop-service-$(IMAGE_VERSION).sh

dockerfile_oltp: deploy_dir
	cp $(TEMPLATES_DIR)/Dockerfile.otlp.prod $(DEPLOY_DIR)

copy_to_remote: check_ssh_env deploy_dir
	scp $(DEPLOY_DIR)/* $(SSH_USER)@index_maker:/home/$(SSH_USER)/$(REMOTE_DIR)

run_build_service: check_ssh_env check_image_version_env
	ssh $(SSH_USER)@index_maker "cd $(REMOTE_DIR) && ./build-service-$(IMAGE_VERSION).sh"

run_stop_service: check_ssh_env check_image_version_env
	ssh $(SSH_USER)@index_maker "cd $(REMOTE_DIR) && ./stop-service.sh"
