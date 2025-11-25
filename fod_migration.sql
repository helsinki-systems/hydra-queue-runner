-- forward
CREATE TABLE FodOutputs (
    build         integer not null,

    timestamp     integer, -- output created
    expectedHash  text,
    actualHash    text,
    foreign key   (build) references Builds(id) on delete cascade
);
ALTER TABLE Builds ADD COLUMN fodCheck boolean NOT NULL DEFAULT false;

-- backwards
DROP TABLE FodOutputs;
ALTER TABLE Builds DROP COLUMN fodCheck;
